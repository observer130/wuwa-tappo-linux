mod geometry;
mod input;
mod menu;
mod pointer;
mod shape;
mod shortcuts;
mod single_instance;
mod socket;
mod storage;
mod system;
mod windows;

use tauri::{
    ipc::{Invoke, InvokeBody},
    Manager,
};

const SYSTEM_COMMANDS: &[&str] = &[
    "write_app_log",
    "get_track_os_info",
    "get_boot_diagnostics",
    "check_app_update",
    "is_devtools_hotkeys_build",
    "is_internal_testing_build",
    "is_elevated_autostart_enabled",
    "set_elevated_autostart",
    "open_app_log_dir",
];

fn handle(invoke: Invoke) -> bool {
    let command = invoke.message.command().to_string();
    if windows::COMMANDS.contains(&command.as_str()) || SYSTEM_COMMANDS.contains(&command.as_str())
    {
        let args = match invoke.message.payload() {
            InvokeBody::Json(value) => value.clone(),
            InvokeBody::Raw(_) => {
                invoke
                    .resolver
                    .reject("Native commands require JSON arguments");
                return true;
            }
        };
        let app = invoke.message.webview().app_handle().clone();
        let label = invoke.message.webview().label().to_string();
        if SYSTEM_COMMANDS.contains(&command.as_str()) {
            invoke.resolver.respond_async(async move {
                tauri::async_runtime::spawn_blocking(move || {
                    system::dispatch(&app, &command, &args)
                        .unwrap_or_else(|| Err(format!("Unhandled system command {command}")))
                })
                .await
                .map_err(|error| tauri::ipc::InvokeError::from(error.to_string()))?
                .map_err(tauri::ipc::InvokeError::from)
            });
        } else {
            let handle = app.clone();
            let resolver = invoke.resolver;
            let fallback = resolver.clone();
            if let Err(error) = app.run_on_main_thread(move || {
                let result = windows::dispatch(&handle, &label, &command, &args);
                resolver.respond(result.map_err(tauri::ipc::InvokeError::from));
            }) {
                fallback.reject(error.to_string());
            }
        }
        return true;
    }
    let handler: fn(Invoke) -> bool = tauri::generate_handler![
        storage::load_key_press_count,
        storage::save_key_press_count,
        storage::load_cumulative_click_count,
        storage::save_cumulative_click_count,
        shortcuts::get_app_shortcuts,
        shortcuts::set_app_shortcuts,
        shortcuts::set_shortcut_capture,
        input::get_input_status,
        input::request_input_access,
        input::resume_input_monitor,
        socket::native_ws_connect,
        socket::native_ws_send,
        socket::native_ws_close,
    ];
    handler(invoke)
}

pub fn run() {
    // This lock covers direct binary launches and desktop entries that bypass
    // run.sh. Keep the guard alive until the event loop exits.
    let instance_lock =
        std::sync::Arc::new(std::sync::Mutex::new(match single_instance::acquire() {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                eprintln!("Wuwa Tappo is already running.");
                return;
            }
            Err(error) => {
                eprintln!("Could not enforce a single instance: {error}");
                return;
            }
        }));
    // Tauri's update restart spawns the replacement process just before this
    // one exits. Release the lock first so the new process can acquire it.
    let exit_lock = instance_lock.clone();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .manage(windows::WindowState::default())
        .manage(input::InputMonitor::new())
        .manage(socket::SocketState::default())
        .invoke_handler(handle)
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(storage::CounterStore::new(
                app.path().app_data_dir()?.join("counters.json"),
            ));
            menu::setup(&handle)?;
            shortcuts::setup(&handle)?;
            app.manage(system::start_network_monitor(handle.clone())?);
            windows::setup(&handle)?;
            if let Err(error) = input::start(handle.clone()) {
                eprintln!("Global input unavailable: {error}");
            }
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(error) => {
            eprintln!("Could not start Wuwa Tappo Linux: {error}");
            return;
        }
    };
    app.run(move |app, event| {
        if let tauri::RunEvent::Exit = event {
            socket::shutdown(app);
            if let Some(monitor) = app.try_state::<system::SystemMonitor>() {
                if let Err(error) = monitor.stop() {
                    eprintln!("Network monitor shutdown failed: {error}");
                }
            }
            if let Err(error) = app.state::<input::InputMonitor>().stop() {
                eprintln!("Input shutdown failed: {error}");
            }
            if let Ok(mut guard) = exit_lock.lock() {
                guard.take();
            }
        }
    });
}
