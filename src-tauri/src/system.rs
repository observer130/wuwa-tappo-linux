use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_opener::OpenerExt;
use url::Url;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MESSAGE_CHARS: usize = 32 * 1024;
const OFFICIAL_API_HOST: &str = "api-mc-wuwatappo.kurogames-global.com";
const OFFICIAL_ASSET_HOST_SUFFIX: &str = ".kurogames-global.com";
const DIAGNOSTIC_CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const DIAGNOSTIC_RUN_HISTORY: usize = 8;
static LOG_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn os_release() -> (String, String) {
    let text = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let mut name = "Linux".to_string();
    let mut version = String::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
            name = value.trim_matches('"').to_string();
        } else if let Some(value) = line.strip_prefix("VERSION_ID=") {
            version = value.trim_matches('"').to_string();
        }
    }
    (name, version)
}

fn is_secret_separator(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'=' | b':' | b'"' | b'\'')
}

fn is_value_terminator(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b'"' | b'\'' | b'&' | b';' | b',' | b'}' | b'>' | b'\\'
        )
}

fn redact(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    let bytes = message.as_bytes();
    let mut ranges = Vec::new();
    for scheme in ["https://", "http://", "wss://", "ws://", "wuwa-buddy://"] {
        let mut from = 0;
        while let Some(offset) = lower[from..].find(scheme) {
            let start = from + offset;
            let end = bytes[start..]
                .iter()
                .position(|byte| is_value_terminator(*byte) && *byte != b'&')
                .map_or(bytes.len(), |offset| start + offset);
            if let Some(offset) = bytes[start..end]
                .iter()
                .position(|b| matches!(b, b'?' | b'#'))
            {
                ranges.push((start + offset, end));
            }
            from = end.max(start + scheme.len());
        }
    }
    for key in [
        "access_token",
        "accesstoken",
        "refresh_token",
        "refreshtoken",
        "kuro_infotoken",
        "infotoken",
        "authorization",
        "cookie",
        "set-cookie",
        "password",
        "passwd",
        "token",
    ] {
        let mut from = 0;
        while let Some(offset) = lower[from..].find(key) {
            let start = from + offset;
            let key_end = start + key.len();
            from = key_end;
            if start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
                continue;
            }
            if key_end < bytes.len()
                && (bytes[key_end].is_ascii_alphanumeric() || bytes[key_end] == b'_')
            {
                continue;
            }
            let mut value_start = key_end;
            let mut separator_seen = false;
            while value_start < bytes.len() && is_secret_separator(bytes[value_start]) {
                separator_seen |= matches!(bytes[value_start], b'=' | b':');
                value_start += 1;
            }
            if !separator_seen {
                continue;
            }
            let value_end = bytes[value_start..]
                .iter()
                .position(|byte| is_value_terminator(*byte))
                .map_or(bytes.len(), |offset| value_start + offset);
            if value_end > value_start {
                ranges.push((value_start, value_end));
            }
        }
    }
    let mut from = 0;
    while let Some(offset) = lower[from..].find("bearer ") {
        let start = from + offset + "bearer ".len();
        let end = bytes[start..]
            .iter()
            .position(|byte| is_value_terminator(*byte))
            .map_or(bytes.len(), |offset| start + offset);
        if end > start {
            ranges.push((start, end));
        }
        from = end.max(start);
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    let mut safe = String::with_capacity(message.len());
    let mut cursor = 0;
    for (start, end) in merged {
        safe.push_str(&message[cursor..start]);
        safe.push_str("[REDACTED]");
        cursor = end;
    }
    safe.push_str(&message[cursor..]);
    safe.replace('\r', "\\r").replace('\n', "\\n")
}

fn write_log_file(directory: &Path, level: &str, message: &str) -> Result<(), String> {
    if !matches!(level, "trace" | "debug" | "info" | "warn" | "error") {
        return Err("unsupported log level".into());
    }
    let safe = redact(message);
    let safe: String = safe.chars().take(MAX_MESSAGE_CHARS).collect();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let line = format!("{timestamp} {level} {safe}\n");
    let _guard = LOG_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "log lock poisoned")?;
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let path = directory.join("app.log");
    match fs::metadata(&path) {
        Ok(metadata) if metadata.len() + line.len() as u64 > MAX_LOG_BYTES => {
            fs::rename(&path, directory.join("app.log.1")).map_err(|e| e.to_string())?;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .map_err(|e| e.to_string())
}

pub fn write_message(app: &AppHandle, level: &str, message: &str) -> Result<(), String> {
    let directory = app.path().app_log_dir().map_err(|e| e.to_string())?;
    write_log_file(&directory, level, message)
}

fn boot_diagnostics(app: &AppHandle) -> Value {
    let (os, version) = os_release();
    let displays = app
        .available_monitors()
        .map(|monitors| {
            monitors
                .iter()
                .map(|m| json!({"device_name": m.name(), "name": m.name()}))
                .collect::<Vec<_>>()
        })
        .ok();
    json!({
        "system": {
            "os": "Linux",
            "os_version": version,
            "os_build": null,
            "architecture": std::env::consts::ARCH,
            "model": os,
        },
        "runtime": {
            "webview_engine": "WebKitGTK",
            "webview_version": null,
            "tauri": tauri::VERSION,
            "reconstructed_backend": true,
        },
        "cpu": { "name": null, "physical_cores": null, "logical_cores": null },
        "memory": { "total_bytes": null },
        "gpus": null,
        "display_names": displays,
    })
}

fn recent_diagnostic_runs() -> &'static Mutex<VecDeque<String>> {
    static RUNS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn validate_diagnostic_url(raw: &str, api: bool) -> Result<String, String> {
    let url =
        Url::parse(raw.trim()).map_err(|_| "network diagnostic url is invalid".to_string())?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err("network diagnostic url must be an https origin".into());
    }
    let host = url.host_str().ok_or("network diagnostic url has no host")?;
    if api {
        if host != OFFICIAL_API_HOST {
            return Err("network diagnostic api host is not the official endpoint".into());
        }
    } else if host != OFFICIAL_API_HOST && !host.ends_with(OFFICIAL_ASSET_HOST_SUFFIX) {
        return Err("network diagnostic assets host is not an official endpoint".into());
    }
    Ok(host.to_owned())
}

async fn probe_diagnostic_host(host: &str) -> &'static str {
    match tokio::time::timeout(
        DIAGNOSTIC_CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((host, 443u16)),
    )
    .await
    {
        Ok(Ok(stream)) => {
            drop(stream);
            "complete"
        }
        Ok(Err(_)) => "network_error",
        Err(_) => "timeout",
    }
}

/// Remember a completed probe so a frontend retry reports `already_collected`.
/// Failed runs stay retryable: the frontend reuses a run id only when it did not
/// receive a result, and a transient outage must be able to recover.
fn remember_diagnostic_run(runs: &mut VecDeque<String>, run_id: &str, status: &str) {
    if status != "complete" {
        return;
    }
    runs.push_back(run_id.to_owned());
    while runs.len() > DIAGNOSTIC_RUN_HISTORY {
        runs.pop_front();
    }
}

/// Probe the official API and asset hosts for the settings diagnostics panel.
/// The frontend retries the same run id, so successful results are memoized per
/// run id to avoid repeating probes and to report `already_collected` on the
/// retry. Failed probes are not memoized so a retry can still recover.
#[tauri::command]
pub async fn run_network_diagnostics(
    app: AppHandle,
    region: String,
    api_base: String,
    assets_host: Option<String>,
    pre_release: bool,
    run_id: String,
) -> Result<Value, String> {
    let api_host = validate_diagnostic_url(&api_base, true)?;
    let assets_host = match assets_host.as_deref() {
        Some(raw) if !raw.trim().is_empty() => Some(validate_diagnostic_url(raw, false)?),
        _ => None,
    };
    let region = region.trim().to_owned();
    if region.is_empty() {
        return Err("network diagnostic region is required".into());
    }
    let run_id = run_id.trim().to_owned();
    if run_id.is_empty() || run_id.len() > 128 {
        return Err("network diagnostic run id is invalid".into());
    }
    {
        let runs = recent_diagnostic_runs()
            .lock()
            .map_err(|_| "network diagnostic state poisoned")?;
        if runs.iter().any(|seen| seen == &run_id) {
            return Ok(json!({ "status": "already_collected" }));
        }
    }
    let api_status = probe_diagnostic_host(&api_host).await;
    let assets_status = match assets_host.as_deref() {
        Some(host) => Some(probe_diagnostic_host(host).await),
        None => None,
    };
    let status = if api_status != "complete" {
        api_status
    } else {
        assets_status.unwrap_or("complete")
    };
    let _ = write_message(
        &app,
        "info",
        &format!(
            "[NETWORK] diagnostics run={run_id} region={region} pre_release={pre_release} \
             api={api_host}:{api_status} assets={assets_status:?}"
        ),
    );
    let mut result = json!({ "status": status });
    if status == "complete" {
        if let Some(host) = assets_host {
            result["resource_url"] =
                json!(format!("https://{host}/app/json-host/latest_host.json"));
            result["resource_source"] = json!("native_assets_probe");
        }
    }
    if let Ok(mut runs) = recent_diagnostic_runs().lock() {
        remember_diagnostic_run(&mut runs, &run_id, status);
    }
    Ok(result)
}

pub struct SystemMonitor {
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl SystemMonitor {
    pub fn stop(&self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self
            .worker
            .lock()
            .map_err(|_| "network monitor lock poisoned")?
            .take()
        {
            worker.join().map_err(|_| "network monitor panicked")?;
        }
        Ok(())
    }
}

/// The official frontend only needs to know whether the process believes it is
/// online. Connectivity failures surface through the real HTTP/Socket.IO calls,
/// so this monitor reports `internetAccess: true` once at startup.
pub fn start_network_monitor(app: AppHandle) -> Result<SystemMonitor, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let worker = thread::Builder::new()
        .name("wuwa-network".into())
        .spawn(move || {
            let mut reported = false;
            while !stop_thread.load(Ordering::Acquire) {
                if !reported {
                    if let Err(error) = app.emit(
                        "cat-native-network-status-changed",
                        json!({"internetAccess": true}),
                    ) {
                        eprintln!("native network event failed: {error}");
                    }
                    reported = true;
                }
                // Sleep in short slices so shutdown does not wait for the full
                // status interval.
                for _ in 0..50 {
                    if stop_thread.load(Ordering::Acquire) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(SystemMonitor {
        stop,
        worker: Mutex::new(Some(worker)),
    })
}

pub fn dispatch(app: &AppHandle, command: &str, args: &Value) -> Option<Result<Value, String>> {
    let result = match command {
        "get_track_os_info" => {
            let (_, version) = os_release();
            Ok(json!({"os": "Linux", "osVersion": version}))
        }
        "get_boot_diagnostics" => Ok(boot_diagnostics(app)),
        "write_app_log" => {
            let level = args.get("level").and_then(Value::as_str);
            let message = args.get("message").and_then(Value::as_str);
            match (level, message) {
                (Some(level), Some(message)) => {
                    write_message(app, level, message).map(|_| Value::Null)
                }
                _ => Err("write_app_log requires string level and message".into()),
            }
        }
        // The Linux rebuild has no signed updater; the UI falls back to "latest".
        "check_app_update" => Ok(Value::Null),
        "is_devtools_hotkeys_build" => Ok(json!(false)),
        "is_internal_testing_build" => Ok(json!(false)),
        "is_elevated_autostart_enabled" => app
            .autolaunch()
            .is_enabled()
            .map(Value::from)
            .map_err(|e| e.to_string()),
        "set_elevated_autostart" => match args.get("enabled").and_then(Value::as_bool) {
            Some(true) => app
                .autolaunch()
                .enable()
                .map(|_| Value::Null)
                .map_err(|e| e.to_string()),
            Some(false) => app
                .autolaunch()
                .disable()
                .map(|_| Value::Null)
                .map_err(|e| e.to_string()),
            None => Err("set_elevated_autostart requires boolean enabled".into()),
        },
        "open_app_log_dir" => (|| -> Result<Value, String> {
            let directory = app.path().app_log_dir().map_err(|e| e.to_string())?;
            fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
            app.opener()
                .open_path(directory.to_string_lossy().into_owned(), None::<&str>)
                .map(|_| Value::Null)
                .map_err(|e| e.to_string())
        })(),
        _ => return None,
    };
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_covers_camel_case_credentials_and_websocket_queries() {
        let safe = redact(
            "accessToken=abc-123 refreshToken: def-456 Cookie: kuro_infoToken=ghi-789 \
             wss://host/socket.io/?token=jkl-012&roomNo=42",
        );
        for secret in ["abc-123", "def-456", "ghi-789", "jkl-012"] {
            assert!(!safe.contains(secret), "{secret} leaked: {safe}");
        }
        assert!(safe.contains("[REDACTED]"));
    }

    #[test]
    fn redaction_keeps_url_paths_and_benign_words() {
        let safe = redact("GET https://example.com/path?secret=1 tokenization is fine");
        assert!(safe.contains("https://example.com/path"));
        assert!(safe.contains("[REDACTED]"));
        assert!(!safe.contains("secret=1"));
        assert!(safe.contains("tokenization is fine"));
    }

    #[test]
    fn redaction_escapes_line_breaks() {
        assert_eq!(redact("a\nb"), "a\\nb");
    }

    #[test]
    fn diagnostics_accept_only_official_https_hosts() {
        assert_eq!(
            validate_diagnostic_url("https://api-mc-wuwatappo.kurogames-global.com", true).unwrap(),
            "api-mc-wuwatappo.kurogames-global.com"
        );
        assert_eq!(
            validate_diagnostic_url("https://mc-wuwatappo.kurogames-global.com", false).unwrap(),
            "mc-wuwatappo.kurogames-global.com"
        );
        assert!(
            validate_diagnostic_url("http://api-mc-wuwatappo.kurogames-global.com", true).is_err()
        );
        assert!(validate_diagnostic_url("https://example.com", true).is_err());
        assert!(validate_diagnostic_url("https://example.com", false).is_err());
        assert!(
            validate_diagnostic_url("https://api-mc-wuwatappo.kurogames-global.com", false).is_ok()
        );
    }

    #[test]
    fn diagnostics_memoize_only_complete_runs() {
        let mut runs = VecDeque::new();
        remember_diagnostic_run(&mut runs, "timeout-run", "timeout");
        assert!(runs.is_empty());
        remember_diagnostic_run(&mut runs, "complete-run", "complete");
        assert_eq!(runs.len(), 1);
        for index in 0..DIAGNOSTIC_RUN_HISTORY + 2 {
            remember_diagnostic_run(&mut runs, &format!("run-{index}"), "complete");
        }
        assert_eq!(runs.len(), DIAGNOSTIC_RUN_HISTORY);
        assert_eq!(runs.back().map(String::as_str), Some("run-9"));
    }
}
