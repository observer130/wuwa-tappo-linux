use serde::Serialize;
use std::io::Read;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    mem::size_of,
    os::fd::AsRawFd,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InputStatus {
    pub granted: bool,
    pub running: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct MousePosition {
    pub x: f64,
    pub y: f64,
    #[serde(rename = "memberId", skip_serializing_if = "Option::is_none")]
    pub member_id: Option<u64>,
}

/// CSS-space offset for the synthetic pointer-leave payload. It is deliberately
/// far from the origin so a stale cached window position (the official frontend
/// caches `innerPosition`/scale) still maps to a negative viewport coordinate.
pub const POINTER_LEAVE_CSS_OFFSET: f64 = -100000.0;

/// Payload that tells the official frontend the pointer left the pet. The
/// frontend converts a `mouse-move` payload back to viewport CSS coordinates
/// with `(payload - windowOrigin) / tauriScale`, so derive the payload from the
/// live window origin and scale instead of hardcoding a screen position: the
/// result stays outside every hit region even when the window origin is
/// negative (a monitor left of or above the primary one).
pub fn pointer_leave_position(origin_x: f64, origin_y: f64, tauri_scale: f64) -> MousePosition {
    let scale = if tauri_scale.is_finite() && tauri_scale > 0.0 {
        tauri_scale
    } else {
        1.0
    };
    MousePosition {
        x: origin_x + POINTER_LEAVE_CSS_OFFSET * scale,
        y: origin_y + POINTER_LEAVE_CSS_OFFSET * scale,
        member_id: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct MouseButton {
    button: &'static str,
    action: &'static str,
    #[serde(rename = "memberId", skip_serializing_if = "Option::is_none")]
    member_id: Option<u64>,
}

type MemberResolver = Arc<dyn Fn(f64, f64) -> Option<u64> + Send + Sync>;
type AuxiliaryHitTest = Arc<dyn Fn(f64, f64) -> bool + Send + Sync>;

struct MonitorState {
    running: AtomicBool,
    granted: AtomicBool,
    stop: AtomicBool,
    pointer_resync: AtomicBool,
    pointer_inside: AtomicBool,
    /// Bumped for every evdev pointer motion (relative or absolute).
    pointer_motion_epoch: AtomicU64,
    /// The motion epoch at the last GDK leave. While it equals
    /// `pointer_motion_epoch` the pointer has not moved since that leave, so
    /// XWayland's frozen sample still equals the true position.
    pointer_leave_motion_epoch: AtomicU64,
    /// Latest CSS pointer sample inside the main window, frozen or live. The
    /// GTK thread reads it in `apply_main_input_shape` to witness region
    /// transitions the 33ms poll can step over.
    pointer_css_sample: Mutex<Option<(f64, f64)>>,
    /// Whether, since the last GDK leave, the input shape was observed to
    /// not cover the pointer sample. A region that re-appears around a
    /// parked pointer only re-enters after such an uncovered moment: a
    /// leave caused by a physical exit leaves the frozen sample inside the
    /// unchanged pet rect, and that sample must not re-open the hover.
    pointer_uncovered_since_leave: AtomicBool,
    pointer_emit_lock: Mutex<()>,
    error: Mutex<Option<String>>,
    member_resolver: Mutex<Option<MemberResolver>>,
    auxiliary_hit_test: Mutex<Option<AuxiliaryHitTest>>,
}

/// Sentinel for "no GDK leave has been processed yet". Until the first leave,
/// XWayland's reported position can be stale from before the window mapped,
/// so the region re-enter check must not trust it.
const POINTER_LEAVE_EPOCH_UNSET: u64 = u64::MAX;

impl MonitorState {
    fn error(&self, message: String) {
        if let Ok(mut error) = self.error.lock() {
            *error = Some(message);
        }
    }

    fn status(&self) -> InputStatus {
        let running = self.running.load(Ordering::Acquire);
        let granted = self.granted.load(Ordering::Acquire);
        let error = if running {
            None
        } else {
            self.error
                .lock()
                .ok()
                .and_then(|error| error.clone())
                .or_else(|| Some("input_monitor_not_running".into()))
        };
        InputStatus {
            granted,
            running,
            error,
        }
    }
}

pub struct InputMonitor {
    state: Arc<MonitorState>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Default for InputMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl InputMonitor {
    pub fn new() -> Self {
        Self {
            state: Arc::new(MonitorState {
                running: AtomicBool::new(false),
                granted: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                pointer_resync: AtomicBool::new(false),
                pointer_inside: AtomicBool::new(false),
                pointer_motion_epoch: AtomicU64::new(0),
                pointer_leave_motion_epoch: AtomicU64::new(POINTER_LEAVE_EPOCH_UNSET),
                pointer_css_sample: Mutex::new(None),
                pointer_uncovered_since_leave: AtomicBool::new(false),
                pointer_emit_lock: Mutex::new(()),
                error: Mutex::new(None),
                member_resolver: Mutex::new(None),
                auxiliary_hit_test: Mutex::new(None),
            }),
            worker: Mutex::new(None),
        }
    }

    pub fn set_member_resolver<F>(&self, resolver: F) -> Result<(), String>
    where
        F: Fn(f64, f64) -> Option<u64> + Send + Sync + 'static,
    {
        *self
            .state
            .member_resolver
            .lock()
            .map_err(|_| "input_router_lock_poisoned")? = Some(Arc::new(resolver));
        Ok(())
    }

    pub fn set_auxiliary_hit_test<F>(&self, hit_test: F) -> Result<(), String>
    where
        F: Fn(f64, f64) -> bool + Send + Sync + 'static,
    {
        *self
            .state
            .auxiliary_hit_test
            .lock()
            .map_err(|_| "input_auxiliary_hit_test_lock_poisoned")? = Some(Arc::new(hit_test));
        Ok(())
    }

    pub fn status(&self) -> InputStatus {
        self.state.status()
    }

    /// Whether the pointer is currently inside the shaped input region. Outside
    /// it XWayland only reports the frozen last-inside position, so polled
    /// coordinates must not drive hover, drag or click hit-testing.
    pub fn pointer_inside(&self) -> bool {
        self.state.pointer_inside.load(Ordering::Acquire)
    }

    /// Latest pointer sample in main-window CSS coordinates, frozen or live.
    pub(crate) fn pointer_sample(&self) -> Option<(f64, f64)> {
        self.state.pointer_css_sample.lock().ok().and_then(|s| *s)
    }

    /// Witness that the current input shape does not cover the pointer sample.
    /// Called from `apply_main_input_shape` and from the polling loop; a region
    /// that later re-covers a parked pointer may only re-enter after this.
    pub(crate) fn mark_pointer_uncovered(&self) {
        self.state
            .pointer_uncovered_since_leave
            .store(true, Ordering::Release);
    }

    /// The pointer entered the shaped input region. Ask the polling loop to
    /// report the next sample even when its coordinates did not change:
    /// XWayland freezes the pointer position while it is outside the region, so
    /// a re-entry at the same pixel as the leave point would otherwise not end
    /// the cleared hover state.
    pub fn mark_pointer_entered(&self) {
        self.state.pointer_inside.store(true, Ordering::Release);
        self.state.pointer_resync.store(true, Ordering::Release);
    }

    /// The pointer left the shaped input region. Cancel a pending forced
    /// resync: when an enter is immediately followed by a leave, the forced
    /// report would replay XWayland's frozen last-inside position and re-open
    /// the hover that the leave handler has just cleared. The polling loop also
    /// drops its last sample while outside, so no residual move is emitted.
    pub fn mark_pointer_left(&self) {
        self.state.pointer_inside.store(false, Ordering::Release);
        self.state.pointer_resync.store(false, Ordering::Release);
        // A new observation window: until an uncovered moment is witnessed
        // the frozen sample is assumed to sit inside the unchanged pet rect,
        // and a region that re-covers it must not re-arm the inside state.
        self.state
            .pointer_uncovered_since_leave
            .store(false, Ordering::Release);
        // Snapshot the motion counter. While it stays unchanged the pointer
        // has not moved since this leave, so XWayland's frozen sample equals
        // the true position and can prove a later region re-entry.
        self.state.pointer_leave_motion_epoch.store(
            self.state.pointer_motion_epoch.load(Ordering::Acquire),
            Ordering::Release,
        );
    }

    /// Emit a pointer move. Serialized with the leave sentinel and button
    /// events so a click never observes a move that was produced after it.
    pub(crate) fn emit_pointer_move(&self, app: &AppHandle, position: MousePosition) {
        let _guard = self.state.pointer_emit_lock.lock();
        let _ = app.emit("mouse-move", position);
    }

    fn emit_pointer_button(
        &self,
        app: &AppHandle,
        move_position: MousePosition,
        button: MouseButton,
    ) {
        let _guard = self.state.pointer_emit_lock.lock();
        let _ = app.emit("mouse-move", move_position);
        let _ = app.emit("mouse-button", button);
    }

    pub fn start(&self, app: AppHandle) -> Result<InputStatus, String> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| "input_worker_lock_poisoned")?;
        if self.state.running.load(Ordering::Acquire) {
            return Ok(self.status());
        }
        if let Some(previous) = worker.take() {
            self.state.stop.store(true, Ordering::Release);
            let _ = previous.join();
        }
        self.state.stop.store(false, Ordering::Release);
        *self
            .state
            .error
            .lock()
            .map_err(|_| "input_status_lock_poisoned")? = None;
        let state = self.state.clone();
        let worker_app = app.clone();
        let shortcuts = crate::shortcuts::shared(&app);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name("wuwatappo-input".into())
            .spawn(move || run_monitor(worker_app, state, shortcuts, ready_sender))
            .map_err(|error| format!("input_worker_spawn_failed: {error}"))?;
        *worker = Some(handle);
        drop(worker);
        let ready = ready_receiver
            .recv()
            .map_err(|_| "input_worker_initialization_failed")?;
        if let Err(error) = ready {
            if error == "input_permission_required" {
                self.state.granted.store(false, Ordering::Release);
            }
            self.state.error(error.clone());
            self.state.running.store(false, Ordering::Release);
            let _ = self.emit_status(&app);
            return Err(error);
        }
        self.state.granted.store(true, Ordering::Release);
        self.state.running.store(true, Ordering::Release);
        let status = self.emit_status(&app)?;
        Ok(status)
    }

    fn emit_status(&self, app: &AppHandle) -> Result<InputStatus, String> {
        let status = self.status();
        app.emit("wuwa-input-status", &status)
            .map_err(|error| format!("input_status_delivery_failed: {error}"))?;
        Ok(status)
    }

    pub fn stop(&self) -> Result<(), String> {
        self.state.stop.store(true, Ordering::Release);
        if let Some(worker) = self
            .worker
            .lock()
            .map_err(|_| "input_worker_lock_poisoned")?
            .take()
        {
            worker.join().map_err(|_| "input_worker_panicked")?;
        }
        self.state.running.store(false, Ordering::Release);
        Ok(())
    }
}

impl Drop for InputMonitor {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub fn start(app: AppHandle) -> Result<InputStatus, String> {
    app.state::<InputMonitor>().start(app.clone())
}

#[tauri::command]
pub fn get_input_status(app: AppHandle) -> InputStatus {
    app.state::<InputMonitor>().status()
}

#[tauri::command]
pub fn resume_input_monitor(app: AppHandle) -> Result<InputStatus, String> {
    start(app)
}

#[tauri::command]
pub fn request_input_access(app: AppHandle) -> Result<InputStatus, String> {
    start(app)
}

#[repr(C)]
struct InputEvent {
    time: libc::timeval,
    type_: u16,
    code: u16,
    value: i32,
}

#[repr(C)]
struct PollFd {
    fd: libc::c_int,
    events: libc::c_short,
    revents: libc::c_short,
}

const EV_KEY: u16 = 1;
const EV_SYN: u16 = 0;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RZ: u16 = 0x05;
const ABS_GAS: u16 = 0x09;
const ABS_BRAKE: u16 = 0x0a;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

#[repr(C)]
struct InputAbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

struct Device {
    _path: PathBuf,
    file: File,
    hat: [i32; 2],
    hat_directions: [bool; 4],
    triggers: [bool; 2],
    trigger_ranges: [[i32; 2]; 2],
}

/// Linux exposes controller buttons as EV_KEY codes above the keyboard range.
/// Include joystick, gamepad, and "trigger happy" mappings used by older pads.
fn is_gamepad_button(code: u16) -> bool {
    matches!(code, 0x120..=0x12b | 0x130..=0x13f | 0x220..=0x223 | 0x2c0..=0x2e7)
}

fn hat_axis(code: u16) -> Option<usize> {
    match code {
        ABS_HAT0X => Some(0),
        ABS_HAT0Y => Some(1),
        _ => None,
    }
}

/// Controller triggers are exposed as buttons on some drivers and as analog
/// axes (`ABS_Z` / `ABS_RZ`, occasionally `ABS_GAS` / `ABS_BRAKE`) on most
/// Linux drivers. The official backend reports them as gamepad buttons.
fn trigger_axis(code: u16) -> Option<usize> {
    match code {
        ABS_Z | ABS_GAS => Some(0),
        ABS_RZ | ABS_BRAKE => Some(1),
        _ => None,
    }
}

/// Up, down, left, right. Keeping four independent flags matters for diagonal
/// D-pad input: the official backend emits one press per direction transition.
fn hat_directions(x: i32, y: i32) -> [bool; 4] {
    [y < 0, y > 0, x < 0, x > 0]
}

fn trigger_pressed(value: i32, range: (i32, i32)) -> bool {
    let (minimum, maximum) = range;
    maximum > minimum && value > minimum + (maximum - minimum) / 2
}

/// Returns `Some(true)` for a press and `Some(false)` for a release.
/// evdev value 2 is auto-repeat and must never be reported as a release.
fn key_edge(value: i32) -> Option<bool> {
    match value {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}

/// `EVIOCGABS` is `_IOR('E', 0x40 + axis, struct input_absinfo)`; on x86_64
/// the request encodes a 24-byte payload.
fn abs_range(file: &File, code: u16) -> Option<(i32, i32)> {
    let mut info = InputAbsInfo {
        value: 0,
        minimum: 0,
        maximum: 0,
        fuzz: 0,
        flat: 0,
        resolution: 0,
    };
    let request = 0x8018_4540_u64 + u64::from(code);
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            request as libc::c_ulong,
            &mut info as *mut InputAbsInfo,
        )
    };
    (result == 0 && info.maximum > info.minimum).then_some((info.minimum, info.maximum))
}

fn trigger_ranges(file: &File) -> [[i32; 2]; 2] {
    [
        abs_range(file, ABS_Z)
            .or_else(|| abs_range(file, ABS_GAS))
            .map_or([0, 255], |(minimum, maximum)| [minimum, maximum]),
        abs_range(file, ABS_RZ)
            .or_else(|| abs_range(file, ABS_BRAKE))
            .map_or([0, 255], |(minimum, maximum)| [minimum, maximum]),
    ]
}

fn open_device(path: &PathBuf) -> Option<File> {
    let file = OpenOptions::new().read(true).open(path).ok()?;
    unsafe {
        let flags = libc::fcntl(file.as_raw_fd(), libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    Some(file)
}

fn scan_devices(devices: &mut Vec<Device>, known: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = fs::read_dir("/dev/input") else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("event"))
        {
            continue;
        }
        if known.contains(&path) {
            continue;
        }
        if let Some(file) = open_device(&path) {
            let trigger_ranges = trigger_ranges(&file);
            known.insert(path.clone());
            devices.push(Device {
                _path: path,
                file,
                hat: [0, 0],
                hat_directions: [false; 4],
                triggers: [false; 2],
                trigger_ranges,
            });
        }
    }
}

fn key_label(code: u16) -> String {
    let label = match code {
        1 => "Escape",
        2 => "1",
        3 => "2",
        4 => "3",
        5 => "4",
        6 => "5",
        7 => "6",
        8 => "7",
        9 => "8",
        10 => "9",
        11 => "0",
        12 => "-",
        13 => "=",
        14 => "Backspace",
        15 => "Tab",
        16 => "q",
        17 => "w",
        18 => "e",
        19 => "r",
        20 => "t",
        21 => "y",
        22 => "u",
        23 => "i",
        24 => "o",
        25 => "p",
        26 => "[",
        27 => "]",
        28 => "Enter",
        29 => "ControlLeft",
        30 => "a",
        31 => "s",
        32 => "d",
        33 => "f",
        34 => "g",
        35 => "h",
        36 => "j",
        37 => "k",
        38 => "l",
        39 => ";",
        40 => "'",
        41 => "`",
        42 => "ShiftLeft",
        43 => "\\",
        44 => "z",
        45 => "x",
        46 => "c",
        47 => "v",
        48 => "b",
        49 => "n",
        50 => "m",
        51 => ",",
        52 => ".",
        53 => "/",
        54 => "ShiftRight",
        55 => "NumpadMultiply",
        56 => "AltLeft",
        57 => " ",
        58 => "CapsLock",
        59 => "F1",
        60 => "F2",
        61 => "F3",
        62 => "F4",
        63 => "F5",
        64 => "F6",
        65 => "F7",
        66 => "F8",
        67 => "F9",
        68 => "F10",
        69 => "NumLock",
        70 => "ScrollLock",
        71 => "Numpad7",
        72 => "Numpad8",
        73 => "Numpad9",
        74 => "NumpadSubtract",
        75 => "Numpad4",
        76 => "Numpad5",
        77 => "Numpad6",
        78 => "NumpadAdd",
        79 => "Numpad1",
        80 => "Numpad2",
        81 => "Numpad3",
        82 => "Numpad0",
        83 => "NumpadDecimal",
        87 => "F11",
        88 => "F12",
        96 => "NumpadEnter",
        97 => "ControlRight",
        98 => "NumpadDivide",
        99 => "PrintScreen",
        100 => "AltRight",
        102 => "Home",
        103 => "ArrowUp",
        104 => "PageUp",
        105 => "ArrowLeft",
        106 => "ArrowRight",
        107 => "End",
        108 => "ArrowDown",
        109 => "PageDown",
        110 => "Insert",
        111 => "Delete",
        113 => "AudioVolumeMute",
        114 => "AudioVolumeDown",
        115 => "AudioVolumeUp",
        119 => "Pause",
        125 => "MetaLeft",
        126 => "MetaRight",
        127 => "ContextMenu",
        _ => return format!("Key{code}"),
    };
    label.to_owned()
}

fn mouse_sample(
    app: &AppHandle,
    pointer: Option<&crate::pointer::PointerReader>,
) -> Option<(MousePosition, f64, f64)> {
    let window = app.get_webview_window("main")?;
    let tauri_scale = window.scale_factor().ok()?.max(f64::EPSILON);
    let dpr = crate::windows::webview_scale(app);
    let (cursor_x, cursor_y) = pointer
        .and_then(crate::pointer::PointerReader::position)
        .or_else(|| {
            window
                .cursor_position()
                .ok()
                .map(|position| (position.x, position.y))
        })?;
    let origin = window.inner_position().ok()?;
    // The official frontend converts the payload with Tauri's scale factor and
    // inner position. Convert physical -> CSS with WebKit's real DPR, then back
    // to the payload coordinate space the frontend expects.
    let (css_x, css_y) =
        crate::geometry::physical_to_css(cursor_x, cursor_y, origin.x as f64, origin.y as f64, dpr);
    let (x, y) = crate::geometry::css_to_payload(
        css_x,
        css_y,
        origin.x as f64,
        origin.y as f64,
        tauri_scale,
    );
    let state = app.state::<InputMonitor>();
    // Share the CSS sample with the GTK thread: `apply_main_input_shape`
    // witnesses fast region transitions against it.
    if let Ok(mut sample) = state.state.pointer_css_sample.lock() {
        *sample = Some((css_x, css_y));
    }
    let member_id = state
        .state
        .member_resolver
        .lock()
        .ok()
        .and_then(|resolver| resolver.as_ref().and_then(|resolve| resolve(x, y)));
    // Auxiliary windows store their regions in global CSS coordinates.
    let global_css_x = origin.x as f64 / dpr + css_x;
    let global_css_y = origin.y as f64 / dpr + css_y;
    Some((
        MousePosition { x, y, member_id },
        global_css_x,
        global_css_y,
    ))
}

fn over_auxiliary(app: &AppHandle, css_x: f64, css_y: f64) -> bool {
    let state = app.state::<InputMonitor>();
    state
        .state
        .auxiliary_hit_test
        .lock()
        .ok()
        .and_then(|hit| hit.as_ref().map(|f| f(css_x, css_y)))
        .unwrap_or(false)
}

/// The dock drag and native click forwarding temporarily make the whole window
/// interactive (`swallow`), so live coordinates are valid even when the pointer
/// is outside the normal hit regions.
fn pointer_swallow_active(app: &AppHandle) -> bool {
    app.try_state::<crate::windows::WindowState>()
        .map(|state| state.swallow_active())
        .unwrap_or(false)
}

/// The synthetic out-of-region position used when the pointer is outside the
/// shaped input region. Derived from the live window origin/scale so it stays
/// outside every hit region even on monitors with negative coordinates.
pub(crate) fn pointer_leave_payload(app: &AppHandle) -> MousePosition {
    let (origin_x, origin_y, scale) = app
        .get_webview_window("main")
        .map(|window| {
            let scale = window
                .scale_factor()
                .ok()
                .filter(|scale| scale.is_finite() && *scale > 0.0)
                .unwrap_or(1.0);
            match window.inner_position().ok() {
                Some(position) => (position.x as f64, position.y as f64, scale),
                None => (0.0, 0.0, scale),
            }
        })
        .unwrap_or((0.0, 0.0, 1.0));
    pointer_leave_position(origin_x, origin_y, scale)
}

/// Button events still drive the global click counter, but a press that lands
/// outside the shaped input region must not be interpreted as a UI hit. Only
/// the pointer-inside state (or an active swallow) may receive live coordinates.
fn button_targets_ui(pointer_inside: bool, swallow: bool) -> bool {
    pointer_inside || swallow
}

/// Whether the "no pointer motion since the leave" proof holds, the sample
/// lands inside the current input shape, and the shape went through an
/// uncovered moment since the leave. KWin only re-evaluates a window's
/// pointer focus when the pointer physically moves, so a region that
/// appears around a stationary pointer never generates EnterNotify; the
/// polling loop uses this to re-arm the inside state instead. The
/// uncovered-moment requirement separates a genuine region rebuild from a
/// physical exit: after a physical exit the frozen sample stays inside the
/// unchanged pet rect, and no region ever stopped covering it.
fn update_reenter_proof(state: &MonitorState, app: &AppHandle, css_x: f64, css_y: f64) -> bool {
    let covered = crate::windows::main_shape_covers(app, css_x, css_y);
    if !covered {
        // Witness an uncovered sample while outside: this leave window has
        // seen the shape stop covering the pointer, so a later re-covering
        // may re-enter.
        state
            .pointer_uncovered_since_leave
            .store(true, Ordering::Release);
    }
    pointer_reenter_proven(
        state.pointer_motion_epoch.load(Ordering::Acquire),
        state.pointer_leave_motion_epoch.load(Ordering::Acquire),
        covered,
        state.pointer_uncovered_since_leave.load(Ordering::Acquire),
    )
}

fn pointer_reenter_proven(
    motion_epoch: u64,
    leave_motion_epoch: u64,
    covered: bool,
    uncovered_since_leave: bool,
) -> bool {
    leave_motion_epoch != POINTER_LEAVE_EPOCH_UNSET
        && motion_epoch == leave_motion_epoch
        && covered
        && uncovered_since_leave
}

/// The official frontend keeps the shortcut table in the native layer. The same
/// evdev stream that counts key presses therefore also drives `app-shortcut`.
fn dispatch_shortcut(
    app: &AppHandle,
    shortcuts: &crate::shortcuts::Shortcuts,
    held: &BTreeSet<u16>,
    key: Option<u32>,
) {
    let Some(key) = key else {
        return;
    };
    let modifiers = crate::shortcuts::modifiers_from(held);
    if let Some(action) = shortcuts.action_for(key, modifiers) {
        // If the window was hidden into the tray, F9 must be able to restore
        // it even on desktops whose tray host does not render AppIndicator
        // icons. Keep the pet's own visibility state untouched in that case.
        if action == "togglePet" && crate::menu::hidden_to_tray() {
            crate::menu::show_main(app);
            return;
        }
        let _ = app.emit("app-shortcut", action);
    }
}

fn run_monitor(
    app: AppHandle,
    state: Arc<MonitorState>,
    shortcuts: crate::shortcuts::Shortcuts,
    ready: mpsc::SyncSender<Result<(), String>>,
) {
    let mut devices = Vec::new();
    let mut known = BTreeSet::new();
    let pointer = crate::pointer::PointerReader::new();
    let layout = crate::layout::KeyLayout::new();
    if layout.is_none() {
        eprintln!("[input] xkb keymap unavailable; using the US fallback mapping");
    }
    scan_devices(&mut devices, &mut known);
    if devices.is_empty() {
        let error = "input_permission_required".to_owned();
        state.error(error.clone());
        let _ = ready.send(Err(error));
        return;
    }
    let _ = ready.send(Ok(()));

    let mut pressed_keys = BTreeSet::new();
    // Modifier state for shortcut matching. Unlike `pressed_keys`, this ignores
    // auto-repeat so a held Ctrl cannot drop out of a chord.
    let mut held_keys = BTreeSet::new();
    let mut pressed_buttons: BTreeMap<&'static str, bool> = BTreeMap::new();
    let mut pressed_gamepad_buttons = BTreeSet::new();
    let mut last_scan = Instant::now();
    let mut last_position: Option<(f64, f64)> = None;
    let mut last_move_emit = Instant::now() - Duration::from_millis(100);
    let mut buffer = [0u8; size_of::<InputEvent>() * 64];

    while !state.stop.load(Ordering::Acquire) {
        if last_scan.elapsed() > Duration::from_secs(2) {
            scan_devices(&mut devices, &mut known);
            last_scan = Instant::now();
        }
        let mut pollfds: Vec<PollFd> = devices
            .iter()
            .map(|device| PollFd {
                fd: device.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        let polled = unsafe {
            libc::poll(
                pollfds.as_mut_ptr() as *mut libc::pollfd,
                pollfds.len() as libc::nfds_t,
                16,
            )
        };
        if polled > 0 {
            let mut stale = Vec::new();
            for (index, pollfd) in pollfds.iter().enumerate() {
                if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    stale.push(index);
                    continue;
                }
                if pollfd.revents & libc::POLLIN == 0 {
                    continue;
                }
                let bytes = match devices[index].file.read(&mut buffer) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                    // A keyboard/mouse was unplugged. Forget the path so the
                    // next scan can reopen it if the device comes back.
                    Err(_) => {
                        stale.push(index);
                        continue;
                    }
                };
                for offset in (0..bytes.saturating_sub(size_of::<InputEvent>() - 1))
                    .step_by(size_of::<InputEvent>())
                {
                    let event = unsafe {
                        std::ptr::read_unaligned(buffer.as_ptr().add(offset) as *const InputEvent)
                    };
                    if event.type_ == EV_SYN {
                        continue;
                    }
                    if event.type_ == EV_REL {
                        // Any pointer motion invalidates the "frozen sample is
                        // still true" assumption behind the region re-enter
                        // check, so every relative packet bumps the epoch.
                        if matches!(event.code, REL_X | REL_Y) {
                            state.pointer_motion_epoch.fetch_add(1, Ordering::AcqRel);
                        }
                        continue;
                    }
                    if event.type_ == EV_ABS {
                        // Absolute pointer devices (touchscreens, tablets)
                        // report position as ABS_X/ABS_Y. Gamepad left sticks
                        // reuse the same codes; a stray bump only suppresses
                        // the re-enter check, which is the safe direction.
                        if matches!(event.code, ABS_X | ABS_Y) {
                            state.pointer_motion_epoch.fetch_add(1, Ordering::AcqRel);
                            continue;
                        }
                        if let Some(axis) = hat_axis(event.code) {
                            let device = &mut devices[index];
                            device.hat[axis] = event.value;
                            let directions = hat_directions(device.hat[0], device.hat[1]);
                            for (direction, pressed) in directions.iter().enumerate() {
                                if *pressed && !device.hat_directions[direction] {
                                    let _ = app.emit("gamepad-button-press", ());
                                }
                            }
                            device.hat_directions = directions;
                        } else if let Some(trigger) = trigger_axis(event.code) {
                            let device = &mut devices[index];
                            let range = (
                                device.trigger_ranges[trigger][0],
                                device.trigger_ranges[trigger][1],
                            );
                            let pressed = trigger_pressed(event.value, range);
                            if pressed && !device.triggers[trigger] {
                                let _ = app.emit("gamepad-button-press", ());
                            }
                            device.triggers[trigger] = pressed;
                        }
                        continue;
                    }
                    if event.type_ != EV_KEY {
                        continue;
                    }
                    if (272..=274).contains(&event.code) {
                        let button = match event.code {
                            272 => "left",
                            273 => "right",
                            _ => "middle",
                        };
                        let pressed = event.value == 1;
                        if pressed && pressed_buttons.contains_key(button) {
                            continue;
                        }
                        let Some((position, css_x, css_y)) = mouse_sample(&app, pointer.as_ref())
                        else {
                            continue;
                        };
                        let auxiliary = over_auxiliary(&app, css_x, css_y);
                        let main_target = if pressed {
                            let main = !auxiliary;
                            pressed_buttons.insert(button, main);
                            main
                        } else {
                            pressed_buttons.remove(button).unwrap_or(false)
                        };
                        let monitor = app.state::<InputMonitor>();
                        let inside = monitor.pointer_inside();
                        let swallow = pointer_swallow_active(&app);
                        let ui_target = button_targets_ui(inside, swallow);
                        let action = if pressed { "press" } else { "release" };
                        let message = format!(
                            "[MOUSE] {button} {action} payload=({:.0},{:.0}) css=({:.0},{:.0}) aux={auxiliary} target={main_target} inside={inside} ui={ui_target}",
                            position.x, position.y, css_x, css_y
                        );
                        let _ = crate::system::write_message(&app, "info", &message);
                        if !main_target {
                            continue;
                        }
                        // Outside the shaped input region the sampled position is
                        // XWayland's frozen last-inside coordinate. Keep sending the
                        // button for the global click counter, but replace the
                        // companion move with an out-of-region sentinel so the
                        // official frontend cannot hit-test the pet or reward UI.
                        let move_position = if ui_target {
                            position
                        } else {
                            pointer_leave_payload(&app)
                        };
                        let payload = MouseButton {
                            button,
                            action,
                            member_id: if ui_target { position.member_id } else { None },
                        };
                        monitor.emit_pointer_button(&app, move_position, payload);
                    } else if is_gamepad_button(event.code) {
                        if event.value == 1 {
                            if pressed_gamepad_buttons.insert(event.code) {
                                let _ = app.emit("gamepad-button-press", ());
                            }
                        } else if event.value == 0 {
                            pressed_gamepad_buttons.remove(&event.code);
                        }
                    } else if event.code < 256 {
                        // evdev value 2 is auto-repeat. The official backend
                        // only releases a key on the matching key-up event.
                        let Some(down) = key_edge(event.value) else {
                            continue;
                        };
                        if down {
                            held_keys.insert(event.code);
                        } else {
                            held_keys.remove(&event.code);
                        }
                        if down {
                            if !pressed_keys.insert(event.code) {
                                continue;
                            }
                            let key_code = if crate::shortcuts::is_modifier(event.code) {
                                None
                            } else {
                                layout
                                    .as_ref()
                                    .and_then(|layout| layout.dom_key_code(event.code))
                                    .or_else(|| crate::shortcuts::dom_key_code(event.code))
                            };
                            dispatch_shortcut(&app, &shortcuts, &held_keys, key_code);
                        } else if !pressed_keys.remove(&event.code) {
                            continue;
                        }
                        let label = layout
                            .as_ref()
                            .and_then(|layout| layout.label(event.code))
                            .unwrap_or_else(|| key_label(event.code));
                        let _ = app.emit(if down { "key-press" } else { "key-release" }, label);
                    }
                }
            }
            for index in stale.into_iter().rev() {
                if index < devices.len() {
                    let device = devices.remove(index);
                    known.remove(&device._path);
                    // Releases from the removed device will never arrive.
                    pressed_keys.clear();
                    held_keys.clear();
                    pressed_buttons.clear();
                    pressed_gamepad_buttons.clear();
                }
            }
        }
        // Cursor movement is polled so it also works for absolute/touchpad devices.
        if state.pointer_resync.swap(false, Ordering::AcqRel)
            && state.pointer_inside.load(Ordering::Acquire)
        {
            last_position = None;
        }
        if !state.pointer_inside.load(Ordering::Acquire) && !pointer_swallow_active(&app) {
            // Outside the shaped input region XWayland reports the frozen
            // last-inside coordinate while the pointer travels over native
            // Wayland surfaces, and stays live while it is over other X
            // windows. A frozen sample must never drive hover or drag: the GDK
            // leave already emitted one out-of-region sentinel that closed
            // the hover, and nothing else is sent until a change proves the
            // sample live. The frozen quiet also keeps the frontend's idle
            // semantics intact (auto-hide can time out).
            if last_move_emit.elapsed() >= Duration::from_millis(33) {
                last_move_emit = Instant::now();
                if let Some((position, css_x, css_y)) = mouse_sample(&app, pointer.as_ref()) {
                    if over_auxiliary(&app, css_x, css_y) {
                        last_position = None;
                    } else if update_reenter_proof(&state, &app, css_x, css_y) {
                        // No motion since the leave, the sample is the true
                        // position, it lands on the current shape, and the
                        // shape went through an uncovered moment since the
                        // leave: a region rebuilt around the parked pointer.
                        // Re-arm the inside state (resync forces the next
                        // sample out) because KWin will not.
                        state.pointer_inside.store(true, Ordering::Release);
                        state.pointer_resync.store(true, Ordering::Release);
                        last_position = None;
                        let _ = crate::system::write_message(
                            &app,
                            "info",
                            "[POINTER] re-entered hit region without motion",
                        );
                    } else if let Some((last_x, last_y)) = last_position {
                        if (last_x - position.x).abs() > 0.01 || (last_y - position.y).abs() > 0.01
                        {
                            // The sample is live: the pointer travels over
                            // another X window. Report the true position like
                            // the official global hook would.
                            last_position = Some((position.x, position.y));
                            app.state::<InputMonitor>()
                                .emit_pointer_move(&app, position);
                        }
                    } else {
                        // First sample after a leave proves nothing about
                        // liveness; record it without emitting.
                        last_position = Some((position.x, position.y));
                    }
                }
            }
        } else if last_move_emit.elapsed() >= Duration::from_millis(33) {
            if let Some((position, css_x, css_y)) = mouse_sample(&app, pointer.as_ref()) {
                if !over_auxiliary(&app, css_x, css_y) {
                    let changed = last_position
                        .map(|(x, y)| {
                            (x - position.x).abs() > 0.01 || (y - position.y).abs() > 0.01
                        })
                        .unwrap_or(true);
                    if changed {
                        last_position = Some((position.x, position.y));
                        app.state::<InputMonitor>()
                            .emit_pointer_move(&app, position);
                    }
                } else {
                    last_position = None;
                }
                last_move_emit = Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_leave_payload_maps_outside_the_viewport() {
        for (origin_x, origin_y, scale) in
            [(0.0, 0.0, 1.0), (2560.0, 0.0, 1.5), (-3840.0, -120.0, 1.25)]
        {
            let payload = pointer_leave_position(origin_x, origin_y, scale);
            let css_x = (payload.x - origin_x) / scale;
            let css_y = (payload.y - origin_y) / scale;
            assert!(
                (css_x - POINTER_LEAVE_CSS_OFFSET).abs() < 1e-6,
                "{origin_x}/{scale}"
            );
            assert!(
                (css_y - POINTER_LEAVE_CSS_OFFSET).abs() < 1e-6,
                "{origin_y}/{scale}"
            );
            assert!(css_x < 0.0 && css_y < 0.0);
            assert!(payload.member_id.is_none());
        }
        let serialized =
            serde_json::to_value(pointer_leave_position(0.0, 0.0, 1.0)).expect("serialize leave");
        assert!(serialized.get("memberId").is_none());
    }

    #[test]
    fn pointer_leave_payload_survives_a_stale_window_origin() {
        // The official frontend caches innerPosition/scale; a leave right after
        // a monitor switch may still be converted with the previous origin.
        let scale = 1.5;
        let payload = pointer_leave_position(2560.0, 0.0, scale);
        for stale_origin_x in [-5120.0, -2560.0, 0.0, 2560.0, 5120.0] {
            assert!(
                (payload.x - stale_origin_x) / scale < 0.0,
                "{stale_origin_x}"
            );
            assert!((payload.y - 0.0) / scale < 0.0);
        }
    }

    #[test]
    fn pointer_crossing_state_gates_ui_events() {
        let monitor = InputMonitor::new();
        assert!(!monitor.pointer_inside());
        assert!(!monitor.state.pointer_resync.load(Ordering::Acquire));

        monitor.mark_pointer_entered();
        assert!(monitor.pointer_inside());
        assert!(monitor.state.pointer_resync.swap(false, Ordering::AcqRel));
        assert!(button_targets_ui(true, false));

        monitor.mark_pointer_left();
        assert!(!monitor.pointer_inside());
        assert!(!monitor.state.pointer_resync.load(Ordering::Acquire));
        assert!(!button_targets_ui(false, false));
        assert!(
            button_targets_ui(false, true),
            "swallow keeps live coordinates"
        );

        monitor.mark_pointer_entered();
        assert!(monitor.state.pointer_resync.swap(false, Ordering::AcqRel));
    }

    #[test]
    fn mark_pointer_left_snapshots_the_motion_epoch() {
        let monitor = InputMonitor::new();
        assert_eq!(
            monitor
                .state
                .pointer_leave_motion_epoch
                .load(Ordering::Acquire),
            POINTER_LEAVE_EPOCH_UNSET
        );
        monitor
            .state
            .pointer_motion_epoch
            .store(3, Ordering::Release);
        monitor.mark_pointer_left();
        assert_eq!(
            monitor
                .state
                .pointer_leave_motion_epoch
                .load(Ordering::Acquire),
            3
        );
        // No motion since the leave, the sample landed on the rebuilt region,
        // and the shape went through an uncovered moment: the sample equals
        // the true position, so the re-entry proof holds.
        assert!(pointer_reenter_proven(
            monitor.state.pointer_motion_epoch.load(Ordering::Acquire),
            monitor
                .state
                .pointer_leave_motion_epoch
                .load(Ordering::Acquire),
            true,
            true
        ));
        // The same frozen sample without an uncovered moment is a plain
        // physical exit: the region never stopped covering the frozen point.
        assert!(!pointer_reenter_proven(
            monitor.state.pointer_motion_epoch.load(Ordering::Acquire),
            monitor
                .state
                .pointer_leave_motion_epoch
                .load(Ordering::Acquire),
            true,
            false
        ));
        // The next pointer motion breaks the proof until the next leave.
        monitor
            .state
            .pointer_motion_epoch
            .fetch_add(1, Ordering::AcqRel);
        assert!(!pointer_reenter_proven(
            monitor.state.pointer_motion_epoch.load(Ordering::Acquire),
            monitor
                .state
                .pointer_leave_motion_epoch
                .load(Ordering::Acquire),
            true,
            true
        ));
    }

    #[test]
    fn pointer_uncovered_witness_resets_on_the_next_leave() {
        let monitor = InputMonitor::new();
        assert!(!monitor
            .state
            .pointer_uncovered_since_leave
            .load(Ordering::Acquire));
        monitor.mark_pointer_uncovered();
        assert!(monitor
            .state
            .pointer_uncovered_since_leave
            .load(Ordering::Acquire));
        // A new leave opens a new observation window.
        monitor.mark_pointer_left();
        assert!(!monitor
            .state
            .pointer_uncovered_since_leave
            .load(Ordering::Acquire));
    }

    #[test]
    fn pointer_reenter_requires_a_leave_coverage_and_an_uncovered_moment() {
        // Before any leave the sentinel epoch blocks the check: a boot-time
        // frozen sample may predate the window and must not re-arm inside.
        assert!(!pointer_reenter_proven(
            0,
            POINTER_LEAVE_EPOCH_UNSET,
            true,
            true
        ));
        assert!(!pointer_reenter_proven(0, 0, false, true));
        // A physical exit freezes inside the unchanged pet rect: no
        // uncovered moment means the region never rebuilt under the pointer.
        assert!(!pointer_reenter_proven(0, 0, true, false));
        assert!(pointer_reenter_proven(0, 0, true, true));
        // Motion after the leave: the sample may be stale, so only a real
        // crossing event may re-arm the inside state.
        assert!(!pointer_reenter_proven(1, 0, true, true));
    }

    #[test]
    fn gamepad_buttons_stay_out_of_keyboard_and_mouse_ranges() {
        for code in [0x120, 0x12b, 0x130, 0x13f, 0x220, 0x223, 0x2c0, 0x2e7] {
            assert!(is_gamepad_button(code), "expected gamepad button {code}");
        }
        for code in [1, 65, 272, 274, 0x100, 0x110, 0x119, 0x224, 0x2e8] {
            assert!(!is_gamepad_button(code), "unexpected gamepad button {code}");
        }
    }

    #[test]
    fn hat_axes_cover_both_dpad_directions() {
        assert_eq!(hat_axis(ABS_HAT0X), Some(0));
        assert_eq!(hat_axis(ABS_HAT0Y), Some(1));
        assert_eq!(hat_axis(0x00), None);
    }

    #[test]
    fn dpad_diagonals_emit_each_direction_transition() {
        assert_eq!(hat_directions(0, 0), [false, false, false, false]);
        assert_eq!(hat_directions(1, 0), [false, false, false, true]);
        assert_eq!(hat_directions(1, -1), [true, false, false, true]);
        assert_eq!(hat_directions(-1, 1), [false, true, true, false]);
    }

    #[test]
    fn auto_repeat_is_not_a_key_release() {
        assert_eq!(key_edge(1), Some(true));
        assert_eq!(key_edge(0), Some(false));
        assert_eq!(key_edge(2), None);
        assert_eq!(key_edge(-1), None);
    }

    #[test]
    fn trigger_axes_cover_analog_and_alternate_names() {
        assert_eq!(trigger_axis(ABS_Z), Some(0));
        assert_eq!(trigger_axis(ABS_GAS), Some(0));
        assert_eq!(trigger_axis(ABS_RZ), Some(1));
        assert_eq!(trigger_axis(ABS_BRAKE), Some(1));
        assert_eq!(trigger_axis(ABS_HAT0X), None);
        assert!(!trigger_pressed(64, (0, 255)));
        assert!(trigger_pressed(200, (0, 255)));
        assert!(trigger_pressed(6, (0, 10)));
        assert!(!trigger_pressed(5, (0, 10)));
    }

    #[test]
    fn granted_reports_permission_separately_from_the_worker_state() {
        let state = MonitorState {
            running: AtomicBool::new(false),
            granted: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            pointer_resync: AtomicBool::new(false),
            pointer_inside: AtomicBool::new(false),
            pointer_motion_epoch: AtomicU64::new(0),
            pointer_leave_motion_epoch: AtomicU64::new(POINTER_LEAVE_EPOCH_UNSET),
            pointer_css_sample: Mutex::new(None),
            pointer_uncovered_since_leave: AtomicBool::new(false),
            pointer_emit_lock: Mutex::new(()),
            error: Mutex::new(Some("input_monitor_not_running".into())),
            member_resolver: Mutex::new(None),
            auxiliary_hit_test: Mutex::new(None),
        };
        let status = state.status();
        assert!(status.granted);
        assert!(!status.running);
        assert_eq!(status.error.as_deref(), Some("input_monitor_not_running"));

        state.granted.store(false, Ordering::Release);
        let status = state.status();
        assert!(!status.granted);
        assert!(!status.running);
    }
}
