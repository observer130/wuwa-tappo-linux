//! Native menu bridge: system tray plus the dock context menu.
//!
//! The official Windows build has both a tray icon and a native dock menu. The
//! Linux port keeps the same menu labels/actions from the frontend payload so
//! the visible behaviour stays aligned with the official client.

use gtk::prelude::*;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{
    menu::{
        CheckMenuItem, IsMenuItem, Menu as TauriMenu, MenuEvent, MenuItem, PredefinedMenuItem,
        Submenu,
    },
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Wry,
};

const TRAY_ID: &str = "main-tray";
static HIDDEN_TO_TRAY: AtomicBool = AtomicBool::new(false);
const SCALES: [u32; 6] = [50, 75, 100, 125, 150, 200];
const OPACITIES: [u32; 5] = [100, 80, 60, 40, 20];

#[derive(Clone)]
struct Labels {
    open_bag: String,
    open_atlas: String,
    open_exchange: String,
    open_room: String,
    hide_pet: String,
    show_pet: String,
    tray_settings: String,
    dock_settings: String,
    window_size: String,
    window_passthrough: String,
    game_mode: String,
    window_opacity: String,
    restart: String,
    quit: String,
    tray_tooltip: String,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            open_bag: "背包".into(),
            open_atlas: "图鉴".into(),
            open_exchange: "合成".into(),
            open_room: "联机".into(),
            hide_pet: "隐藏桌宠".into(),
            show_pet: "显示桌宠".into(),
            tray_settings: "设置".into(),
            dock_settings: "设置".into(),
            window_size: "窗口大小".into(),
            window_passthrough: "点击穿透".into(),
            game_mode: "游戏模式".into(),
            window_opacity: "窗口透明度".into(),
            restart: "重启".into(),
            quit: "退出".into(),
            tray_tooltip: "Wuwa Tappo".into(),
        }
    }
}

#[derive(Clone, Copy)]
struct MenuConfig {
    pet_dock_visible: bool,
    pet_scale: u32,
    window_opacity: u32,
    window_passthrough: bool,
    game_mode: bool,
}

impl Default for MenuConfig {
    fn default() -> Self {
        Self {
            pet_dock_visible: true,
            pet_scale: 100,
            window_opacity: 100,
            window_passthrough: false,
            game_mode: false,
        }
    }
}

#[derive(Clone)]
struct TrayItems {
    open_bag: MenuItem<Wry>,
    open_atlas: MenuItem<Wry>,
    open_exchange: MenuItem<Wry>,
    open_room: MenuItem<Wry>,
    pet_visibility: MenuItem<Wry>,
    settings: MenuItem<Wry>,
    window_size: Submenu<Wry>,
    window_passthrough: CheckMenuItem<Wry>,
    game_mode: CheckMenuItem<Wry>,
    window_opacity: Submenu<Wry>,
    restart: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    scale_items: Vec<(u32, CheckMenuItem<Wry>)>,
    opacity_items: Vec<(u32, CheckMenuItem<Wry>)>,
}

#[derive(Default)]
pub struct MenuState {
    labels: Mutex<Labels>,
    config: Mutex<MenuConfig>,
    tray: Mutex<Option<TrayIcon<Wry>>>,
    items: Mutex<Option<TrayItems>>,
}

pub fn setup(app: &AppHandle) -> Result<(), String> {
    app.manage(MenuState::default());
    let state = app.state::<MenuState>();
    let labels = state
        .labels
        .lock()
        .map_err(|_| "menu_labels_lock_poisoned")?
        .clone();
    let config = *state
        .config
        .lock()
        .map_err(|_| "menu_config_lock_poisoned")?;

    if !tray_supported() {
        eprintln!(
            "System tray is unavailable (install libayatana-appindicator3 or libappindicator3); \
             close-to-tray is disabled."
        );
        return Ok(());
    }
    let icon = match tauri::image::Image::from_bytes(include_bytes!("../icons/128x128.png")) {
        Ok(icon) => icon,
        Err(error) => {
            eprintln!("Tray icon is unavailable: {error}");
            return Ok(());
        }
    };

    let (menu, items) = match build_tray_menu(app, &labels, config) {
        Ok(menu) => menu,
        Err(error) => {
            eprintln!("Could not build tray menu: {error}");
            return Ok(());
        }
    };
    let tray = match TrayIconBuilder::<Wry>::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(handle_tray_event)
        .build(app)
    {
        Ok(tray) => tray,
        Err(error) => {
            eprintln!("Could not create system tray: {error}");
            return Ok(());
        }
    };

    if let Ok(mut stored) = state.items.lock() {
        *stored = Some(items);
    }
    if let Ok(mut stored) = state.tray.lock() {
        *stored = Some(tray);
    }
    Ok(())
}

fn tray_supported() -> bool {
    // tray-icon uses libappindicator through dlopen on Linux. Checking the
    // loader cache avoids hiding the main window into a tray that cannot be
    // created on this system.
    let Ok(output) = std::process::Command::new("ldconfig").arg("-p").output() else {
        return true;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    text.contains("libayatana-appindicator3.so") || text.contains("libappindicator3.so")
}

fn build_tray_menu(
    app: &AppHandle,
    labels: &Labels,
    config: MenuConfig,
) -> tauri::Result<(TauriMenu<Wry>, TrayItems)> {
    let item = |id: &str, text: &str| -> tauri::Result<MenuItem<Wry>> {
        MenuItem::with_id(app, id, text, true, None::<&str>)
    };
    let check = |id: &str, text: &str, checked: bool| -> tauri::Result<CheckMenuItem<Wry>> {
        CheckMenuItem::with_id(app, id, text, true, checked, None::<&str>)
    };

    let open_bag = item("tray-open-bag", &labels.open_bag)?;
    let open_atlas = item("tray-open-atlas", &labels.open_atlas)?;
    let open_exchange = item("tray-open-exchange", &labels.open_exchange)?;
    let open_room = item("tray-open-room", &labels.open_room)?;
    let pet_visibility = item(
        "tray-pet-visibility",
        if config.pet_dock_visible {
            &labels.hide_pet
        } else {
            &labels.show_pet
        },
    )?;
    let settings = item("tray-settings", &labels.tray_settings)?;
    let window_passthrough = check(
        "tray-passthrough",
        &labels.window_passthrough,
        config.window_passthrough,
    )?;
    let game_mode = check("tray-game-mode", &labels.game_mode, config.game_mode)?;
    let restart = item("tray-restart", &labels.restart)?;
    let quit = item("tray-quit", &labels.quit)?;

    let scale_items = SCALES
        .iter()
        .map(|value| {
            check(
                &format!("tray-scale-{value}"),
                &format!("{value}%"),
                config.pet_scale == *value,
            )
            .map(|item| (*value, item))
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let scale_refs: Vec<&dyn IsMenuItem<Wry>> = scale_items
        .iter()
        .map(|(_, item)| item as &dyn IsMenuItem<Wry>)
        .collect();
    let window_size = Submenu::with_items(app, &labels.window_size, true, &scale_refs)?;

    let opacity_items = OPACITIES
        .iter()
        .map(|value| {
            check(
                &format!("tray-opacity-{value}"),
                &format!("{value}%"),
                config.window_opacity == *value,
            )
            .map(|item| (*value, item))
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let opacity_refs: Vec<&dyn IsMenuItem<Wry>> = opacity_items
        .iter()
        .map(|(_, item)| item as &dyn IsMenuItem<Wry>)
        .collect();
    let window_opacity = Submenu::with_items(app, &labels.window_opacity, true, &opacity_refs)?;

    let separator = PredefinedMenuItem::separator(app)?;
    let separator_2 = PredefinedMenuItem::separator(app)?;
    let separator_3 = PredefinedMenuItem::separator(app)?;
    let separator_4 = PredefinedMenuItem::separator(app)?;
    let menu = TauriMenu::with_items(
        app,
        &[
            &open_bag,
            &open_atlas,
            &open_exchange,
            &open_room,
            &separator,
            &pet_visibility,
            &settings,
            &separator_2,
            &window_size,
            &window_opacity,
            &separator_3,
            &window_passthrough,
            &game_mode,
            &separator_4,
            &restart,
            &quit,
        ],
    )?;

    Ok((
        menu,
        TrayItems {
            open_bag,
            open_atlas,
            open_exchange,
            open_room,
            pet_visibility,
            settings,
            window_size,
            window_passthrough,
            game_mode,
            window_opacity,
            restart,
            quit,
            scale_items,
            opacity_items,
        },
    ))
}

pub fn sync(app: &AppHandle, args: &Value) -> Result<Value, String> {
    let Some(state) = app.try_state::<MenuState>() else {
        return Ok(Value::Null);
    };
    if let Ok(mut labels) = state.labels.lock() {
        if let Some(values) = args.get("labels").and_then(Value::as_object) {
            update_label(values, "openBag", &mut labels.open_bag);
            update_label(values, "openAtlas", &mut labels.open_atlas);
            update_label(values, "openExchange", &mut labels.open_exchange);
            update_label(values, "openRoom", &mut labels.open_room);
            update_label(values, "hidePet", &mut labels.hide_pet);
            update_label(values, "showPet", &mut labels.show_pet);
            update_label(values, "trayOpenSettings", &mut labels.tray_settings);
            update_label(values, "dockSettings", &mut labels.dock_settings);
            update_label(values, "windowSize", &mut labels.window_size);
            update_label(values, "windowPassthrough", &mut labels.window_passthrough);
            update_label(values, "gameMode", &mut labels.game_mode);
            update_label(values, "windowOpacity", &mut labels.window_opacity);
            update_label(values, "restart", &mut labels.restart);
            update_label(values, "quit", &mut labels.quit);
            update_label(values, "trayTooltip", &mut labels.tray_tooltip);
        }
    }
    if let Ok(mut config) = state.config.lock() {
        if let Some(value) = args.get("petDockVisible").and_then(Value::as_bool) {
            config.pet_dock_visible = value;
        }
        if let Some(value) = args.get("petScale").and_then(Value::as_f64) {
            config.pet_scale = (value.round() as i64).clamp(50, 200) as u32;
        }
        if let Some(value) = args.get("windowOpacity").and_then(Value::as_f64) {
            config.window_opacity = (value.round() as i64).clamp(0, 100) as u32;
        }
        if let Some(value) = args.get("windowPassthrough").and_then(Value::as_bool) {
            config.window_passthrough = value;
        }
        if let Some(value) = args.get("gameMode").and_then(Value::as_bool) {
            config.game_mode = value;
        }
    }
    refresh_tray(app);
    Ok(Value::Null)
}

fn update_label(values: &serde_json::Map<String, Value>, key: &str, target: &mut String) {
    if let Some(value) = values.get(key).and_then(Value::as_str) {
        if !value.is_empty() {
            *target = value.to_owned();
        }
    }
}

fn refresh_tray(app: &AppHandle) {
    let Some(state) = app.try_state::<MenuState>() else {
        return;
    };
    let labels = state
        .labels
        .lock()
        .map(|labels| labels.clone())
        .unwrap_or_default();
    let config = state
        .config
        .lock()
        .map(|config| *config)
        .unwrap_or_default();
    // Clone the handles and release the state lock before touching menu items:
    // menu setters dispatch to the GTK main thread and must not be called while
    // another thread could hold this mutex waiting for the same main loop.
    let items = match state.items.lock() {
        Ok(items) => items.clone(),
        Err(_) => None,
    };
    let Some(items) = items else {
        return;
    };
    let _ = items.open_bag.set_text(&labels.open_bag);
    let _ = items.open_atlas.set_text(&labels.open_atlas);
    let _ = items.open_exchange.set_text(&labels.open_exchange);
    let _ = items.open_room.set_text(&labels.open_room);
    let _ = items.pet_visibility.set_text(if config.pet_dock_visible {
        &labels.hide_pet
    } else {
        &labels.show_pet
    });
    let _ = items.settings.set_text(&labels.tray_settings);
    let _ = items.window_size.set_text(&labels.window_size);
    let _ = items.window_opacity.set_text(&labels.window_opacity);
    let _ = items
        .window_passthrough
        .set_text(&labels.window_passthrough);
    let _ = items.game_mode.set_text(&labels.game_mode);
    let _ = items.restart.set_text(&labels.restart);
    let _ = items.quit.set_text(&labels.quit);
    let _ = items
        .window_passthrough
        .set_checked(config.window_passthrough);
    let _ = items.game_mode.set_checked(config.game_mode);
    for (value, item) in &items.scale_items {
        let _ = item.set_checked(*value == config.pet_scale);
    }
    for (value, item) in &items.opacity_items {
        let _ = item.set_checked(*value == config.window_opacity);
    }
    let tray = match state.tray.lock() {
        Ok(tray) => tray.clone(),
        Err(_) => None,
    };
    if let Some(tray) = tray {
        let _ = tray.set_tooltip(Some(labels.tray_tooltip.as_str()));
    }
}

fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref().to_owned();
    if let Some(value) = id.strip_prefix("tray-scale-") {
        perform_action(app, &format!("scale:{value}"));
        return;
    }
    if let Some(value) = id.strip_prefix("tray-opacity-") {
        perform_action(app, &format!("opacity:{value}"));
        return;
    }
    let action = match id.as_str() {
        "tray-open-bag" => "bag",
        "tray-open-atlas" => "atlas",
        "tray-open-exchange" => "exchange",
        "tray-open-room" => "room",
        "tray-settings" => "setting",
        "tray-pet-visibility" => "toggle-pet",
        "tray-passthrough" => "toggle-passthrough",
        "tray-game-mode" => "toggle-game-mode",
        "tray-restart" => "restart",
        "tray-quit" => "quit",
        _ => return,
    };
    perform_action(app, action);
}

fn handle_tray_event(tray: &TrayIcon<Wry>, event: TrayIconEvent) {
    if matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
    ) {
        show_main(tray.app_handle());
    }
}

pub(crate) fn main_is_visible(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(true)
}

pub(crate) fn hidden_to_tray() -> bool {
    HIDDEN_TO_TRAY.load(Ordering::Acquire)
}

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = window.set_always_on_top(true);
        HIDDEN_TO_TRAY.store(false, Ordering::Release);
    }
}

fn perform_action(app: &AppHandle, action: &str) {
    if let Some(value) = action.strip_prefix("scale:") {
        if let Ok(percent) = value.parse::<u32>() {
            mutate_config(app, |config| config.pet_scale = percent);
            let _ = app.emit("cat-settings-pet-scale", json!({ "petScale": percent }));
            refresh_tray(app);
        }
        return;
    }
    if let Some(value) = action.strip_prefix("opacity:") {
        if let Ok(percent) = value.parse::<u32>() {
            mutate_config(app, |config| config.window_opacity = percent);
            let _ = app.emit("cat-settings-opacity", json!({ "opacityPercent": percent }));
            refresh_tray(app);
        }
        return;
    }
    match action {
        "bag" | "atlas" | "exchange" | "room" => {
            if !main_is_visible(app) {
                show_main(app);
            }
            let _ = app.emit("dock-open-panel", action);
        }
        "setting" => {
            if !main_is_visible(app) {
                show_main(app);
            }
            let _ = app.emit("dock-open-panel", "setting");
        }
        "toggle-pet" => {
            let visible = app
                .try_state::<MenuState>()
                .and_then(|state| {
                    state
                        .config
                        .lock()
                        .ok()
                        .map(|config| config.pet_dock_visible)
                })
                .unwrap_or(true);
            let window_shown = main_is_visible(app);
            if !window_shown {
                show_main(app);
            }
            if window_shown || !visible {
                let _ = app.emit("tray-toggle-pet-visibility", ());
            }
            mutate_config(app, |config| config.pet_dock_visible = !visible);
            refresh_tray(app);
        }
        "toggle-passthrough" => {
            let _ = app.emit("tray-toggle-window-passthrough", ());
            mutate_config(app, |config| {
                config.window_passthrough = !config.window_passthrough;
            });
            refresh_tray(app);
        }
        "toggle-game-mode" => {
            let _ = app.emit("tray-toggle-game-mode", ());
            mutate_config(app, |config| config.game_mode = !config.game_mode);
            refresh_tray(app);
        }
        "restart" => {
            let _ = app.emit("tray-restart", ());
        }
        "quit" => {
            let _ = app.emit("tray-quit", ());
        }
        _ => {}
    }
}

fn mutate_config(app: &AppHandle, mutate: impl FnOnce(&mut MenuConfig)) {
    if let Some(state) = app.try_state::<MenuState>() {
        if let Ok(mut config) = state.config.lock() {
            mutate(&mut config);
        }
    }
}

pub fn hide_main_to_tray(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<MenuState>() else {
        return false;
    };
    let has_tray = state
        .tray
        .lock()
        .map(|tray| tray.is_some())
        .unwrap_or(false);
    if !has_tray {
        return false;
    }
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    match window.hide() {
        Ok(()) => {
            HIDDEN_TO_TRAY.store(true, Ordering::Release);
            true
        }
        Err(error) => {
            eprintln!("Could not hide main window into the tray: {error}");
            false
        }
    }
}

pub fn popup(
    app: &AppHandle,
    _window: &tauri::WebviewWindow,
    x: f64,
    y: f64,
) -> Result<Value, String> {
    // Mirrors the official backend's debug event so the frontend console can
    // confirm that the native menu path was reached.
    let _ = app.emit("dock-context-menu-debug", json!({ "x": x, "y": y }));
    let (labels, config) = match app.try_state::<MenuState>() {
        Some(state) => (
            state
                .labels
                .lock()
                .map(|labels| labels.clone())
                .unwrap_or_default(),
            state
                .config
                .lock()
                .map(|config| *config)
                .unwrap_or_default(),
        ),
        None => (Labels::default(), MenuConfig::default()),
    };
    let menu = gtk::Menu::new();
    add_item(&menu, &labels.open_bag, app, "bag");
    add_item(&menu, &labels.open_atlas, app, "atlas");
    add_item(&menu, &labels.open_exchange, app, "exchange");
    add_item(&menu, &labels.open_room, app, "room");
    menu.append(&gtk::SeparatorMenuItem::new());
    add_item(
        &menu,
        if config.pet_dock_visible {
            &labels.hide_pet
        } else {
            &labels.show_pet
        },
        app,
        "toggle-pet",
    );
    add_item(&menu, &labels.dock_settings, app, "setting");
    menu.append(&gtk::SeparatorMenuItem::new());
    add_choice_submenu(
        &menu,
        &labels.window_size,
        app,
        "scale",
        &SCALES,
        config.pet_scale,
    );
    add_choice_submenu(
        &menu,
        &labels.window_opacity,
        app,
        "opacity",
        &OPACITIES,
        config.window_opacity,
    );
    menu.append(&gtk::SeparatorMenuItem::new());
    add_check_item(
        &menu,
        &labels.window_passthrough,
        app,
        "toggle-passthrough",
        config.window_passthrough,
    );
    add_check_item(
        &menu,
        &labels.game_mode,
        app,
        "toggle-game-mode",
        config.game_mode,
    );
    menu.append(&gtk::SeparatorMenuItem::new());
    add_item(&menu, &labels.restart, app, "restart");
    add_item(&menu, &labels.quit, app, "quit");
    menu.show_all();
    let menu = std::rc::Rc::new(menu);
    let menu_for_hide = menu.clone();
    menu.connect_hide(move |_| {
        // The menu is kept alive by Rc until it is hidden on the GTK main
        // thread, which is the only place `destroy` is safe to call.
        unsafe {
            menu_for_hide.destroy();
        }
    });
    menu.popup_at_pointer(None);
    Ok(Value::Null)
}

fn add_item(menu: &gtk::Menu, label: &str, app: &AppHandle, action: &'static str) {
    let item = gtk::MenuItem::with_label(label);
    let app = app.clone();
    item.connect_activate(move |_| perform_action(&app, action));
    menu.append(&item);
}

fn add_check_item(
    menu: &gtk::Menu,
    label: &str,
    app: &AppHandle,
    action: &'static str,
    checked: bool,
) {
    let item = gtk::CheckMenuItem::with_label(label);
    item.set_active(checked);
    let app = app.clone();
    item.connect_activate(move |_| perform_action(&app, action));
    menu.append(&item);
}

fn add_choice_submenu(
    menu: &gtk::Menu,
    label: &str,
    app: &AppHandle,
    prefix: &'static str,
    values: &[u32],
    selected: u32,
) {
    let item = gtk::MenuItem::with_label(label);
    let submenu = gtk::Menu::new();
    for value in values {
        let value = *value;
        let child = gtk::CheckMenuItem::with_label(&format!("{value}%"));
        child.set_active(value == selected);
        let app = app.clone();
        child.connect_activate(move |_| perform_action(&app, &format!("{prefix}:{value}")));
        submenu.append(&child);
    }
    item.set_submenu(Some(&submenu));
    menu.append(&item);
}
