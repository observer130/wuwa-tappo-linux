use crate::geometry::{work_area_insets, HitRegion};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, RwLock,
    },
};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use x11rb::protocol::xproto::Rectangle;

pub const COMMANDS: &[&str] = &[
    "apply_main_window_position",
    "close_about_window",
    "close_login_window",
    "close_oss_notice_window",
    "close_panel_window",
    "close_update_window",
    "ensure_main_taskbar_icon",
    "exit_app",
    "exit_maintenance_app",
    "finish_main_window_monitor_switch",
    "get_app_update_detail",
    "get_main_desktop_safe_area",
    "get_main_taskbar_insets",
    "get_maintenance_notice_payload",
    "has_maintenance_notice",
    "hide_main_after_maintenance_notice_ready",
    "is_force_update_locked",
    "open_about_window",
    "open_login_window",
    "open_maintenance_notice",
    "open_oss_notice_window",
    "open_panel_window",
    "open_update_window",
    "popup_main_dock_context_menu",
    "report_webview_metrics",
    "raise_panel_window_above_pets",
    "resize_login_window_to_content",
    "set_main_force_cursor_ignore",
    "set_main_hit_region",
    "set_main_skip_taskbar",
    "set_main_swallow_mouse",
    "set_main_window_content_bounds",
    "set_main_window_fullscreen_shell",
    "set_main_window_monitor_bounds",
    "set_native_input_cursor",
    "set_overlay_cursor",
    "set_panel_window_collapsed",
    "show_toast_overlay",
    "sync_aux_windows_to_main_monitor",
    "sync_main_window_to_current_monitor",
    "sync_native_menu",
    "login_window_ready",
];

#[derive(Default)]
pub struct WindowState {
    hit: RwLock<HitState>,
    update: Mutex<Value>,
    maintenance: Mutex<Option<Value>>,
    panel_collapsed: Mutex<bool>,
    pending_login: Mutex<Option<Value>>,
    login_ready: AtomicBool,
    auxiliary: RwLock<BTreeMap<String, HitRegion>>,
    main_geometry: RwLock<MainGeometry>,
    hit_synced: AtomicBool,
    hit_last: Mutex<Option<(usize, usize)>>,
    webview_scale: RwLock<Option<f64>>,
    shape: Mutex<Option<crate::shape::InputShape>>,
    shape_last: Mutex<Option<(u8, usize)>>,
    stop: AtomicBool,
}

#[derive(Default)]
struct MainGeometry {
    left: f64,
    top: f64,
    scale: f64,
}

#[derive(Default)]
struct HitState {
    regions: Vec<HitRegion>,
    input_regions: Vec<HitRegion>,
    window_passthrough: bool,
    force_ignore: bool,
    swallow: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HitUpdate {
    regions: Vec<HitRegion>,
    #[serde(default)]
    input_regions: Vec<HitRegion>,
    #[serde(default)]
    update_native_input: bool,
    #[serde(default)]
    window_passthrough: bool,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn main_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    app.get_webview_window("main")
        .ok_or_else(|| "Main window is unavailable".into())
}

/// CSS pixels per physical pixel as seen by the embedded WebKit view.
///
/// Tauri/tao reports the X11 monitor scale (often 1.0), while GTK may apply a
/// fractional XSettings scale such as 1.5. Hit regions arrive in CSS pixels, so
/// every pointer comparison has to use the value reported by the webview.
pub fn webview_scale(app: &AppHandle) -> f64 {
    let reported = app
        .try_state::<WindowState>()
        .and_then(|state| state.webview_scale.read().ok().and_then(|value| *value));
    reported
        .filter(|value| value.is_finite() && *value > 0.0)
        .or_else(|| {
            app.get_webview_window("main")
                .and_then(|window| window.scale_factor().ok())
        })
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(1.0)
}

/// Multiplier converting CSS pixels to the logical units Tauri uses for window
/// sizes and positions. When Tauri's X11 scale matches WebKit's DPR this is 1.
fn css_to_logical(app: &AppHandle) -> f64 {
    let tauri_scale = app
        .get_webview_window("main")
        .and_then(|window| window.scale_factor().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(1.0);
    (webview_scale(app) / tauri_scale).max(f64::EPSILON)
}

fn initialize(label: &str) -> Result<String, String> {
    let mut initialization = format!(
        "{}\n{}",
        include_str!("../../scripts/websocket.js"),
        include_str!("../../scripts/platform.js")
    );
    if label == "main" && cfg!(debug_assertions) && std::env::args().any(|arg| arg == "--self-test")
    {
        initialization.push_str(include_str!("../../scripts/runtime-smoke.js"));
    }
    Ok(initialization)
}

fn refresh_geometry(app: &AppHandle, label: &str) -> Result<(), String> {
    let window = app
        .get_webview_window(label)
        .ok_or("Window geometry unavailable")?;
    let scale = window.scale_factor().map_err(err)?;
    let pos = window.inner_position().map_err(err)?;
    let state = app.state::<WindowState>();
    if label == "main" {
        *state.main_geometry.write().map_err(err)? = MainGeometry {
            left: pos.x as f64 / scale,
            top: pos.y as f64 / scale,
            scale,
        };
    } else if label != "toast" {
        let pos = window.outer_position().map_err(err)?;
        let size = window.outer_size().map_err(err)?;
        // Auxiliary hit tests compare against the CSS space the frontend uses.
        let dpr = webview_scale(app);
        state.auxiliary.write().map_err(err)?.insert(
            label.to_string(),
            HitRegion {
                left: pos.x as f64 / dpr,
                top: pos.y as f64 / dpr,
                width: size.width as f64 / dpr,
                height: size.height as f64 / dpr,
                member_id: None,
            },
        );
    }
    Ok(())
}

fn apply_main_input_shape(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<WindowState>();
    let dpr = webview_scale(app);
    let (mode, rectangles) = {
        let hit = state.hit.read().map_err(err)?;
        if hit.swallow {
            let mode = 2;
            let size = main_window(app)?.inner_size().map_err(err)?;
            (
                mode,
                vec![Rectangle {
                    x: 0,
                    y: 0,
                    width: size.width.min(u16::MAX as u32) as u16,
                    height: size.height.min(u16::MAX as u32) as u16,
                }],
            )
        } else if hit.force_ignore {
            (0, Vec::new())
        } else {
            let regions = if hit.window_passthrough {
                &hit.input_regions
            } else {
                &hit.regions
            };
            let mode = 1;
            (
                mode,
                regions
                    .iter()
                    .filter(|region| region.is_valid() && region.width > 0. && region.height > 0.)
                    .map(|region| Rectangle {
                        x: (region.left * dpr).round().clamp(0., i16::MAX as f64) as i16,
                        y: (region.top * dpr).round().clamp(0., i16::MAX as f64) as i16,
                        width: (region.width * dpr).round().clamp(1., u16::MAX as f64) as u16,
                        height: (region.height * dpr).round().clamp(1., u16::MAX as f64) as u16,
                    })
                    .collect(),
            )
        }
    };
    let mut shape = state.shape.lock().map_err(err)?;
    if shape.is_none() {
        *shape = Some(crate::shape::InputShape::for_window(&main_window(app)?)?);
    }
    let result = shape
        .as_ref()
        .ok_or("Input shape is unavailable")?
        .apply(&rectangles);
    match &result {
        Ok(()) => {
            let signature = (mode, rectangles.len());
            let changed = state
                .shape_last
                .lock()
                .map(|mut last| last.replace(signature) != Some(signature))
                .unwrap_or(false);
            if changed {
                let message = format!(
                    "[SHAPE] mode={} rects={}",
                    match mode {
                        2 => "full",
                        1 => "regions",
                        _ => "empty",
                    },
                    rectangles.len()
                );
                let _ = crate::system::write_message(app, "info", &message);
            }
        }
        Err(error) => {
            let message = format!("[SHAPE] apply failed: {error}");
            let _ = crate::system::write_message(app, "warn", &message);
        }
    }
    result
}

fn build_window(
    app: &AppHandle,
    label: &str,
    route: &str,
    size: (f64, f64),
    focus: bool,
) -> Result<WebviewWindow, String> {
    let css_scale = css_to_logical(app);
    let window = WebviewWindowBuilder::new(
        app,
        label,
        WebviewUrl::App(format!("index.html#{route}").into()),
    )
    .title(if label == "main" {
        "Wuwa Tappo CN Linux"
    } else {
        "Wuwa Tappo"
    })
    .initialization_script(initialize(label)?)
    .on_page_load(|window, payload| {
        if matches!(payload.event(), tauri::webview::PageLoadEvent::Started) {
            crate::socket::close_window(window.app_handle(), window.label());
        }
    })
    .inner_size(size.0 * css_scale, size.1 * css_scale)
    // Every official route is a rounded card. An opaque native window leaves
    // its white background visible around those corners (panel/login/about).
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .resizable(label == "panel")
    .maximizable(false)
    .visible(false)
    .focused(focus)
    .focusable(focus)
    .always_on_top(true)
    .disable_drag_drop_handler()
    .build()
    .map_err(err)?;
    if label == "panel" {
        window
            .set_min_size(Some(LogicalSize::new(1100. * css_scale, 760. * css_scale)))
            .map_err(err)?;
    }
    window.on_window_event({
        let app = app.clone();
        let label = label.to_string();
        move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                crate::socket::close_window(&app, &label);
            }
            if matches!(
                event,
                tauri::WindowEvent::Moved(_)
                    | tauri::WindowEvent::Resized(_)
                    | tauri::WindowEvent::ScaleFactorChanged { .. }
            ) {
                let tracked = label == "main"
                    || app
                        .state::<WindowState>()
                        .auxiliary
                        .read()
                        .is_ok_and(|windows| windows.contains_key(&label));
                if tracked {
                    if let Err(error) = refresh_geometry(&app, &label) {
                        eprintln!("Could not refresh {label} geometry: {error}");
                    }
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                // There is no tray icon to restore a hidden pet, so closing the
                // main window must quit instead of leaving an unreachable
                // process holding the single-instance lock.
                if label == "main" {
                    app.state::<WindowState>()
                        .stop
                        .store(true, Ordering::Release);
                    // Tearing down WebKit synchronously from a GTK window
                    // event deadlocks or aborts. Hide first, then terminate
                    // from a separate thread after the event handler returns.
                    // Tauri's graceful exit still runs GTK teardown in this
                    // state, so use the process exit path that leaves no
                    // lingering lock or webview child.
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.hide();
                    }
                    std::thread::spawn(|| {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        std::process::exit(0);
                    });
                    return;
                }
                if let Ok(mut windows) = app.state::<WindowState>().auxiliary.write() {
                    windows.remove(&label);
                }
                if let Some(w) = app.get_webview_window(&label) {
                    if let Err(error) = w.hide() {
                        eprintln!("Could not hide {label}: {error}");
                    }
                }
                if label == "panel" {
                    if let Err(error) = app.emit("cat-panel-window-closed", ()) {
                        eprintln!("Panel close event failed: {error}");
                    }
                }
                if label == "login" {
                    if let Err(error) = app.emit("cat-login-overlay", json!({"open":false})) {
                        eprintln!("Login close event failed: {error}");
                    }
                }
            }
        }
    });
    Ok(window)
}

fn show_aux(
    app: &AppHandle,
    label: &str,
    route: &str,
    size: (f64, f64),
) -> Result<(WebviewWindow, bool), String> {
    let existing = app.get_webview_window(label);
    let existed = existing.is_some();
    let window = match existing {
        Some(w) => w,
        None => build_window(app, label, route, size, true)?,
    };
    window.show().map_err(err)?;
    center_on_main(app, &window, Some(size))?;
    window.set_focus().map_err(err)?;
    refresh_geometry(app, label)?;
    Ok((window, existed))
}

fn center_on_main(
    app: &AppHandle,
    window: &WebviewWindow,
    size: Option<(f64, f64)>,
) -> Result<(), String> {
    let main = main_window(app)?;
    let monitor = main
        .current_monitor()
        .map_err(err)?
        .or(main.primary_monitor().map_err(err)?)
        .ok_or("No display is available")?;
    let dpr = webview_scale(app);
    let physical = match size {
        Some((width, height)) => ((width * dpr).round(), (height * dpr).round()),
        None => {
            let size = window.outer_size().map_err(err)?;
            (size.width as f64, size.height as f64)
        }
    };
    let work = monitor.work_area();
    let target = PhysicalPosition::new(
        (work.position.x as f64 + (work.size.width as f64 - physical.0) / 2.).round() as i32,
        (work.position.y as f64 + (work.size.height as f64 - physical.1) / 2.).round() as i32,
    );
    window.set_position(target).map_err(err)
}

fn fullscreen_shell(app: &AppHandle) -> Result<(), String> {
    let w = main_window(app)?;
    let monitor = w
        .current_monitor()
        .map_err(err)?
        .or(w.primary_monitor().map_err(err)?)
        .ok_or("No display is available")?;
    w.set_position(*monitor.position()).map_err(err)?;
    w.set_size(*monitor.size()).map_err(err)?;
    Ok(())
}

fn safe_area(app: &AppHandle) -> Result<Value, String> {
    let w = main_window(app)?;
    let m = w
        .current_monitor()
        .map_err(err)?
        .ok_or("No current display")?;
    let p = m.position();
    let s = m.size();
    let a = m.work_area();
    let insets = work_area_insets(
        (p.x as f64, p.y as f64, s.width as f64, s.height as f64),
        (
            a.position.x as f64,
            a.position.y as f64,
            a.size.width as f64,
            a.size.height as f64,
        ),
        webview_scale(app),
    );
    Ok(
        json!({"insets": insets, "autoHideEdge": null, "foregroundFullscreen": foreground_fullscreen(app)?}),
    )
}

// The Linux rebuild never inspects other applications' windows. The official
// frontend only needs a boolean here, so report that no foreground fullscreen
// application was detected.
fn foreground_fullscreen(_app: &AppHandle) -> Result<bool, String> {
    Ok(false)
}

fn payload(args: &Value) -> &Value {
    args.get("payload").unwrap_or(args)
}
fn number(args: &Value, name: &str) -> Result<f64, String> {
    args.get(name)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("Invalid {name}"))
}

fn close(app: &AppHandle, label: &str) -> Result<Value, String> {
    app.state::<WindowState>()
        .auxiliary
        .write()
        .map_err(err)?
        .remove(label);
    if let Some(w) = app.get_webview_window(label) {
        w.hide().map_err(err)?;
    }
    if label == "panel" {
        app.emit("cat-panel-window-closed", ()).map_err(err)?;
    }
    if label == "login" {
        app.emit("cat-login-overlay", json!({"open": false}))
            .map_err(err)?;
    }
    Ok(Value::Null)
}

pub fn dispatch(
    app: &AppHandle,
    label: &str,
    command: &str,
    args: &Value,
) -> Result<Value, String> {
    let state = app.state::<WindowState>();
    match command {
        "open_about_window" => {
            let (w, existed) = show_aux(app, "about", "/about", (680., 700.))?;
            if existed {
                w.emit("about-open", ()).map_err(err)?;
            }
        }
        "open_oss_notice_window" => {
            let (w, existed) = show_aux(app, "oss-notice", "/oss-notice", (780., 680.))?;
            if existed {
                w.emit("oss-notice-open", ()).map_err(err)?;
            }
        }
        "open_login_window" => {
            let intent = args
                .get("intent")
                .and_then(Value::as_str)
                .unwrap_or("login");
            if !["login", "show"].contains(&intent) {
                return Err("Unknown login intent".into());
            }
            *state.pending_login.lock().map_err(err)? =
                Some(json!({"intent":intent,"compact":args.get("compact")}));
            let (w, _) = show_aux(app, "login", "/login", (600., 660.))?;
            app.emit("cat-login-overlay", json!({"open": true}))
                .map_err(err)?;
            if state.login_ready.load(Ordering::Acquire) {
                if let Some(pending) = state.pending_login.lock().map_err(err)?.take() {
                    w.emit("login-modal-open", pending).map_err(err)?;
                }
            }
        }
        "login_window_ready" => {
            if label != "login" {
                return Err("Login readiness requires the login window".into());
            }
            state.login_ready.store(true, Ordering::Release);
            if let Some(pending) = state.pending_login.lock().map_err(err)?.take() {
                app.get_webview_window("login")
                    .ok_or("Login window is unavailable")?
                    .emit("login-modal-open", pending)
                    .map_err(err)?;
            }
        }
        "open_panel_window" => {
            let tab = payload(args)
                .get("tab")
                .and_then(Value::as_str)
                .unwrap_or("bag");
            if !["bag", "atlas", "exchange", "room", "setting"].contains(&tab) {
                return Err("Unknown panel tab".into());
            }
            let (w, existed) = show_aux(app, "panel", &format!("/panel?tab={tab}"), (1100., 760.))?;
            if existed {
                w.emit("panel-open-tab", tab).map_err(err)?;
            }
        }
        "open_update_window" => {
            *state.update.lock().map_err(err)? = args.get("detail").cloned().unwrap_or(Value::Null);
            let (w, _) = show_aux(app, "update", "/update", (600., 500.))?;
            w.emit("update-modal-open", ()).map_err(err)?;
            w.emit(
                "cat-app-update-available",
                args.get("detail").cloned().unwrap_or(Value::Null),
            )
            .map_err(err)?;
        }
        "open_maintenance_notice" => {
            let notice = args.get("payload").cloned().filter(|v| !v.is_null());
            *state.maintenance.lock().map_err(err)? = Some(notice.clone().unwrap_or(Value::Null));
            let (w, _) = show_aux(app, "update", "/update", (600., 500.))?;
            w.emit("maintenance-notice-open", notice).map_err(err)?;
        }
        "close_about_window" => return close(app, "about"),
        "close_oss_notice_window" => return close(app, "oss-notice"),
        "close_login_window" => return close(app, "login"),
        "close_panel_window" => return close(app, "panel"),
        "close_update_window" => return close(app, "update"),
        "get_app_update_detail" => return Ok(state.update.lock().map_err(err)?.clone()),
        "get_maintenance_notice_payload" => {
            return Ok(state
                .maintenance
                .lock()
                .map_err(err)?
                .clone()
                .unwrap_or(Value::Null))
        }
        "has_maintenance_notice" => {
            return Ok(json!(state.maintenance.lock().map_err(err)?.is_some()))
        }
        "is_force_update_locked" => {
            let detail = state.update.lock().map_err(err)?;
            return Ok(json!(detail
                .get("forceUpdate")
                .or_else(|| detail.get("forced"))
                .and_then(Value::as_bool)
                .unwrap_or(false)));
        }
        "hide_main_after_maintenance_notice_ready" => main_window(app)?.hide().map_err(err)?,
        "resize_login_window_to_content" => {
            let p = payload(args);
            let width = number(p, "width")?;
            let height = number(p, "height")?;
            if width <= 0. || height <= 0. {
                return Err("Login dimensions must be positive".into());
            }
            let w = app
                .get_webview_window("login")
                .ok_or("Login window is unavailable")?;
            let dpr = webview_scale(app);
            w.set_size(PhysicalSize::new(
                (width * dpr).round() as u32,
                (height * dpr).round() as u32,
            ))
            .map_err(err)?;
            center_on_main(app, &w, Some((width, height)))?;
        }
        "raise_panel_window_above_pets" => {
            let w = app
                .get_webview_window("panel")
                .ok_or("Panel window is unavailable")?;
            w.set_always_on_top(true).map_err(err)?;
            w.set_focus().map_err(err)?;
        }
        "set_panel_window_collapsed" => {
            let w = app
                .get_webview_window("panel")
                .ok_or("Panel window is unavailable")?;
            let collapsed = payload(args)
                .get("collapsed")
                .and_then(Value::as_bool)
                .ok_or("Missing collapsed state")?;
            let mut previous = state.panel_collapsed.lock().map_err(err)?;
            if *previous != collapsed {
                let dpr = webview_scale(app);
                let size = w.inner_size().map_err(err)?;
                let css_width = size.width as f64 / dpr;
                let css_height = size.height as f64 / dpr;
                let min_width = if collapsed { 958. } else { 1100. };
                let min_height = 760.;
                w.set_min_size(Some(PhysicalSize::new(
                    (min_width * dpr).round() as u32,
                    (min_height * dpr).round() as u32,
                )))
                .map_err(err)?;
                let width = (css_width + if collapsed { -142. } else { 142. }).max(min_width);
                w.set_size(PhysicalSize::new(
                    (width * dpr).round() as u32,
                    (css_height.max(min_height) * dpr).round() as u32,
                ))
                .map_err(err)?;
                *previous = collapsed;
            }
        }
        "set_main_hit_region" => {
            let update: HitUpdate = match serde_json::from_value(payload(args).clone()) {
                Ok(update) => update,
                Err(error) => {
                    let message = format!("[HIT] rejected payload: {error}");
                    let _ = crate::system::write_message(app, "warn", &message);
                    return Err(error.to_string());
                }
            };
            if update
                .regions
                .iter()
                .chain(update.input_regions.iter())
                .any(|r| !r.is_valid())
            {
                let _ = crate::system::write_message(
                    app,
                    "warn",
                    "[HIT] rejected payload: invalid region geometry",
                );
                return Err("Invalid hit region geometry".into());
            }
            let mut hit = state.hit.write().map_err(err)?;
            hit.regions = update.regions;
            if update.update_native_input {
                hit.input_regions = update.input_regions;
            }
            hit.window_passthrough = update.window_passthrough;
            hit.force_ignore = false;
            let counts = (hit.regions.len(), hit.input_regions.len());
            let changed = state
                .hit_last
                .lock()
                .map(|mut last| last.replace(counts) != Some(counts))
                .unwrap_or(false);
            if changed {
                let message = format!(
                    "[HIT] regions={} input_regions={} window_passthrough={}",
                    counts.0, counts.1, hit.window_passthrough,
                );
                let _ = crate::system::write_message(app, "info", &message);
            }
            if counts.0 > 0 && !state.hit_synced.swap(true, Ordering::Relaxed) {
                let sample = hit
                    .regions
                    .iter()
                    .take(12)
                    .map(|region| {
                        format!(
                            "({:.0},{:.0},{:.0}x{:.0})",
                            region.left, region.top, region.width, region.height
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let message = format!("[HIT] first non-empty regions: {sample}");
                let _ = crate::system::write_message(app, "info", &message);
            }
            drop(hit);
            apply_main_input_shape(app)?;
        }
        "set_main_force_cursor_ignore" => {
            state.hit.write().map_err(err)?.force_ignore = args
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or("Missing enabled state")?;
            apply_main_input_shape(app)?;
        }
        "set_main_swallow_mouse" => {
            let enabled = args
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or("Missing enabled state")?;
            state.hit.write().map_err(err)?.swallow = enabled;
            apply_main_input_shape(app)?;
        }
        "set_main_window_fullscreen_shell" => fullscreen_shell(app)?,
        "set_main_window_monitor_bounds" => {
            let p = payload(args);
            let x = number(p, "x")?;
            let y = number(p, "y")?;
            let width = number(p, "width")?;
            let height = number(p, "height")?;
            if width <= 0. || height <= 0. {
                return Err("Monitor dimensions must be positive".into());
            }
            let w = main_window(app)?;
            state.hit.write().map_err(err)?.force_ignore = true;
            apply_main_input_shape(app)?;
            let target = w
                .available_monitors()
                .map_err(err)?
                .into_iter()
                .find(|m| {
                    m.position().x == x as i32
                        && m.position().y == y as i32
                        && m.size().width == width as u32
                        && m.size().height == height as u32
                })
                .ok_or("Selected display is no longer available")?;
            let scale = target.scale_factor();
            w.set_position(LogicalPosition::new(x / scale, y / scale))
                .map_err(err)?;
            w.set_size(LogicalSize::new(width / scale, height / scale))
                .map_err(err)?;
        }
        "finish_main_window_monitor_switch" => {
            state.hit.write().map_err(err)?.force_ignore = false;
            apply_main_input_shape(app)?;
        }
        "set_main_window_content_bounds" => {
            let p = payload(args);
            let width = number(p, "width")?;
            let height = number(p, "height")?;
            if width <= 0. || height <= 0. {
                return Err("Window dimensions must be positive".into());
            }
            let w = main_window(app)?;
            let dpr = webview_scale(app);
            if p.get("x").is_some() || p.get("y").is_some() {
                w.set_position(PhysicalPosition::new(
                    (number(p, "x")? * dpr).round() as i32,
                    (number(p, "y")? * dpr).round() as i32,
                ))
                .map_err(err)?;
            }
            w.set_size(PhysicalSize::new(
                (width * dpr).round() as u32,
                (height * dpr).round() as u32,
            ))
            .map_err(err)?;
        }
        "apply_main_window_position" => {
            let top = payload(args)
                .get("alwaysOnTop")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            main_window(app)?.set_always_on_top(top).map_err(err)?;
            // In the original fullscreen shell the Vue dock, not the OS window,
            // applies the selected corner; moving this window would move it twice.
            fullscreen_shell(app)?;
        }
        "sync_main_window_to_current_monitor" => {
            let w = main_window(app)?;
            w.set_visible_on_all_workspaces(true).map_err(err)?;
        }
        "sync_aux_windows_to_main_monitor" => {
            for (name, w) in app.webview_windows() {
                if name != "main" && name != "toast" {
                    center_on_main(app, &w, None)?;
                }
            }
        }
        "report_webview_metrics" => {
            let metrics = payload(args);
            let scale = number(metrics, "devicePixelRatio")?;
            let width = number(metrics, "innerWidth")?;
            let height = number(metrics, "innerHeight")?;
            if !(0.5..=4.0).contains(&scale) || width <= 0. || height <= 0. {
                return Err("Invalid webview metrics".into());
            }
            let changed = {
                let mut current = state.webview_scale.write().map_err(err)?;
                let changed = current.is_none_or(|value| (value - scale).abs() > f64::EPSILON);
                *current = Some(scale);
                changed
            };
            if changed {
                let message =
                    format!("[WEBVIEW] devicePixelRatio={scale} inner={width:.0}x{height:.0}");
                let _ = crate::system::write_message(app, "info", &message);
                // Auxiliary regions are stored in CSS coordinates too; refresh
                // them when the webview scale changes (startup or monitor move).
                for (label, _) in app.webview_windows() {
                    if label != "main" && label != "toast" {
                        let _ = refresh_geometry(app, &label);
                    }
                }
            }
        }
        "get_main_desktop_safe_area" => return safe_area(app),
        "get_main_taskbar_insets" => return Ok(safe_area(app)?["insets"].clone()),
        "set_overlay_cursor" | "set_native_input_cursor" => {
            let kind = args
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("default");
            let cursor = match kind {
                "pointer" => tauri::CursorIcon::Hand,
                "grab" => tauri::CursorIcon::Grab,
                "grabbing" => tauri::CursorIcon::Grabbing,
                "default" => tauri::CursorIcon::Default,
                _ => return Err("Unsupported cursor".into()),
            };
            main_window(app)?.set_cursor_icon(cursor).map_err(err)?;
        }
        "ensure_main_taskbar_icon" => {
            main_window(app)?.set_skip_taskbar(false).map_err(err)?;
        }
        "set_main_skip_taskbar" => {
            let skip = args
                .get("skip")
                .and_then(Value::as_bool)
                .ok_or("Missing skip state")?;
            main_window(app)?.set_skip_taskbar(skip).map_err(err)?;
        }
        "show_toast_overlay" => {
            // The toast is created once during startup so this command never
            // builds a GTK window from inside the event loop. Show it before
            // making it click-through: tao unwraps the GDK window and a hidden
            // window has not been realized yet.
            let w = app
                .get_webview_window("toast")
                .ok_or("Toast window is unavailable")?;
            w.show().map_err(err)?;
            w.set_ignore_cursor_events(true).map_err(err)?;
            center_on_main(app, &w, Some((620., 160.)))?;
        }
        "sync_native_menu" => return crate::menu::sync(app, args),
        "popup_main_dock_context_menu" => {
            return crate::menu::popup(
                app,
                &main_window(app)?,
                number(args, "x")?,
                number(args, "y")?,
            )
        }
        "exit_app" => {
            state.stop.store(true, Ordering::Release);
            app.exit(0);
        }
        // The maintenance notice covers the whole app. Its exit button has no
        // in-app destination, so it closes the process like the original client.
        "exit_maintenance_app" => {
            state.stop.store(true, Ordering::Release);
            app.exit(0);
        }
        _ => return Err(format!("Unsupported native command {command} from {label}")),
    }
    Ok(Value::Null)
}

pub fn setup(app: &AppHandle) -> Result<(), String> {
    let w = build_window(app, "main", "/", (900., 650.), false)?;
    fullscreen_shell(app)?;
    refresh_geometry(app, "main")?;
    w.show().map_err(err)?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    if let Err(error) = apply_main_input_shape(app) {
        eprintln!("Initial input shape failed: {error}");
    }
    build_window(app, "toast", "/toast", (620., 160.), false)?;
    if cfg!(debug_assertions) && std::env::args().any(|arg| arg == "--inspect") {
        w.open_devtools();
    }
    let app_for_input = app.clone();
    app.state::<crate::input::InputMonitor>()
        .set_member_resolver(move |x, y| {
            let state = app_for_input.state::<WindowState>();
            let geometry = state.main_geometry.read().ok()?;
            let (x, y) = (
                x / geometry.scale - geometry.left,
                y / geometry.scale - geometry.top,
            );
            let hit = state.hit.read().ok()?;
            hit.input_regions
                .iter()
                .chain(hit.regions.iter())
                .rev()
                .find(|r| r.contains(x, y))
                .and_then(|r| r.member_id)
        })?;
    let app_for_auxiliary = app.clone();
    app.state::<crate::input::InputMonitor>()
        .set_auxiliary_hit_test(move |x, y| {
            let state = app_for_auxiliary.state::<WindowState>();
            state
                .auxiliary
                .read()
                .is_ok_and(|regions| regions.values().any(|region| region.contains(x, y)))
        })?;
    Ok(())
}
