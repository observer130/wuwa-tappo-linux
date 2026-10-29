use serde::Serialize;
use std::io::Read;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    mem::size_of,
    os::fd::AsRawFd,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
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
    stop: AtomicBool,
    error: Mutex<Option<String>>,
    member_resolver: Mutex<Option<MemberResolver>>,
    auxiliary_hit_test: Mutex<Option<AuxiliaryHitTest>>,
}

impl MonitorState {
    fn error(&self, message: String) {
        if let Ok(mut error) = self.error.lock() {
            *error = Some(message);
        }
    }

    fn status(&self) -> InputStatus {
        let running = self.running.load(Ordering::Acquire);
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
            granted: running,
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
                stop: AtomicBool::new(false),
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
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let handle = thread::Builder::new()
            .name("wuwatappo-input".into())
            .spawn(move || run_monitor(worker_app, state, ready_sender))
            .map_err(|error| format!("input_worker_spawn_failed: {error}"))?;
        *worker = Some(handle);
        drop(worker);
        let ready = ready_receiver
            .recv()
            .map_err(|_| "input_worker_initialization_failed")?;
        if let Err(error) = ready {
            self.state.error(error.clone());
            self.state.running.store(false, Ordering::Release);
            let _ = self.emit_status(&app);
            return Err(error);
        }
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
const EV_ABS: u16 = 3;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

struct Device {
    _path: PathBuf,
    file: File,
    hat: [i32; 2],
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
            known.insert(path.clone());
            devices.push(Device {
                _path: path,
                file,
                hat: [0, 0],
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

fn run_monitor(
    app: AppHandle,
    state: Arc<MonitorState>,
    ready: mpsc::SyncSender<Result<(), String>>,
) {
    let mut devices = Vec::new();
    let mut known = BTreeSet::new();
    let pointer = crate::pointer::PointerReader::new();
    scan_devices(&mut devices, &mut known);
    if devices.is_empty() {
        let error = "input_permission_required".to_owned();
        state.error(error.clone());
        let _ = ready.send(Err(error));
        return;
    }
    let _ = ready.send(Ok(()));

    let mut pressed_keys = BTreeSet::new();
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
                    if event.type_ == EV_ABS {
                        let Some(axis) = hat_axis(event.code) else {
                            continue;
                        };
                        let device = &mut devices[index];
                        let was_active = device.hat != [0, 0];
                        device.hat[axis] = event.value;
                        let active = device.hat != [0, 0];
                        if !was_active && active {
                            let _ = app.emit("gamepad-button-press", ());
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
                        let action = if pressed { "press" } else { "release" };
                        let message = format!(
                            "[MOUSE] {button} {action} payload=({:.0},{:.0}) css=({:.0},{:.0}) aux={auxiliary} target={main_target}",
                            position.x, position.y, css_x, css_y
                        );
                        let _ = crate::system::write_message(&app, "info", &message);
                        if !main_target {
                            continue;
                        }
                        let _ = app.emit("mouse-move", position);
                        let payload = MouseButton {
                            button,
                            action,
                            member_id: position.member_id,
                        };
                        let _ = app.emit("mouse-button", payload);
                    } else if is_gamepad_button(event.code) {
                        if event.value == 1 {
                            if pressed_gamepad_buttons.insert(event.code) {
                                let _ = app.emit("gamepad-button-press", ());
                            }
                        } else if event.value == 0 {
                            pressed_gamepad_buttons.remove(&event.code);
                        }
                    } else if event.code < 256 {
                        let down = event.value == 1;
                        if down {
                            if !pressed_keys.insert(event.code) {
                                continue;
                            }
                        } else if !pressed_keys.remove(&event.code) {
                            continue;
                        }
                        let _ = app.emit(
                            if down { "key-press" } else { "key-release" },
                            key_label(event.code),
                        );
                    }
                }
            }
            for index in stale.into_iter().rev() {
                if index < devices.len() {
                    let device = devices.remove(index);
                    known.remove(&device._path);
                    // Releases from the removed device will never arrive.
                    pressed_keys.clear();
                    pressed_buttons.clear();
                    pressed_gamepad_buttons.clear();
                }
            }
        }
        // Cursor movement is polled so it also works for absolute/touchpad devices.
        if last_move_emit.elapsed() >= Duration::from_millis(33) {
            if let Some((position, css_x, css_y)) = mouse_sample(&app, pointer.as_ref()) {
                if !over_auxiliary(&app, css_x, css_y) {
                    let changed = last_position
                        .map(|(x, y)| {
                            (x - position.x).abs() > 0.01 || (y - position.y).abs() > 0.01
                        })
                        .unwrap_or(true);
                    if changed {
                        last_position = Some((position.x, position.y));
                        let _ = app.emit("mouse-move", position);
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
}
