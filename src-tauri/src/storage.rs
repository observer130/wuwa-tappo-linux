use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

pub const MAX_SAFE_COUNT: u64 = 9_007_199_254_740_991;
static WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CumulativeClickCount {
    pub count: u64,
    pub cuid: String,
    pub environment: String,
}

#[derive(Default, Serialize, Deserialize)]
struct EnvironmentCounters {
    key_press: u64,
    active_cuid: Option<String>,
    cumulative: BTreeMap<String, u64>,
}

#[derive(Serialize, Deserialize)]
struct CounterData {
    version: u32,
    environments: BTreeMap<String, EnvironmentCounters>,
    /// Cumulative totals captured right before a local reset, so a later login
    /// can restore the gross click count the user already accumulated. Keyed
    /// by environment and then by account cuid.
    #[serde(default)]
    reset_archive: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Default for CounterData {
    fn default() -> Self {
        Self {
            version: 1,
            environments: BTreeMap::new(),
            reset_archive: BTreeMap::new(),
        }
    }
}

pub struct CounterStore {
    path: PathBuf,
    transaction: Mutex<()>,
}

impl CounterStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            transaction: Mutex::new(()),
        }
    }

    pub fn load_key_press(&self, environment: &str) -> Result<u64, String> {
        validate_environment(environment)?;
        let _transaction = self
            .transaction
            .lock()
            .map_err(|_| "counter_store_lock_poisoned")?;
        Ok(self
            .read()?
            .environments
            .get(environment)
            .map_or(0, |entry| entry.key_press))
    }

    pub fn save_key_press(&self, environment: &str, count: u64) -> Result<(), String> {
        validate_environment(environment)?;
        validate_count(count)?;
        let _transaction = self
            .transaction
            .lock()
            .map_err(|_| "counter_store_lock_poisoned")?;
        let mut data = self.read()?;
        data.environments
            .entry(environment.to_owned())
            .or_default()
            .key_press = count;
        self.write(&data)
    }

    pub fn load_cumulative(&self, environment: &str) -> Result<CumulativeClickCount, String> {
        validate_environment(environment)?;
        let _transaction = self
            .transaction
            .lock()
            .map_err(|_| "counter_store_lock_poisoned")?;
        let data = self.read()?;
        let entry = data.environments.get(environment);
        let cuid = entry
            .and_then(|entry| entry.active_cuid.as_deref())
            .unwrap_or("");
        let count = entry
            .and_then(|entry| entry.cumulative.get(cuid))
            .copied()
            .unwrap_or(0);
        Ok(CumulativeClickCount {
            count,
            cuid: cuid.to_owned(),
            environment: environment.to_owned(),
        })
    }

    pub fn save_cumulative(&self, environment: &str, cuid: &str, count: u64) -> Result<(), String> {
        validate_environment(environment)?;
        validate_count(count)?;
        if cuid.trim().is_empty() {
            return Err("counter_account_required".into());
        }
        let _transaction = self
            .transaction
            .lock()
            .map_err(|_| "counter_store_lock_poisoned")?;
        let mut data = self.read()?;
        let entry = data.environments.entry(environment.to_owned()).or_default();
        entry.cumulative.insert(cuid.to_owned(), count);
        entry.active_cuid = Some(cuid.to_owned());
        self.write(&data)
    }

    /// Cumulative total archived for `cuid` right before the local data was
    /// reset, or 0 when the account has never reset on this machine.
    pub fn load_reset_archive(&self, environment: &str, cuid: &str) -> Result<u64, String> {
        validate_environment(environment)?;
        let _transaction = self
            .transaction
            .lock()
            .map_err(|_| "counter_store_lock_poisoned")?;
        Ok(self
            .read()?
            .reset_archive
            .get(environment)
            .and_then(|archive| archive.get(cuid))
            .copied()
            .unwrap_or(0))
    }

    /// Record the cumulative total before a reset. An empty cuid (the user is
    /// logged out) is skipped: there is no account the total can be restored
    /// to after the reset.
    pub fn save_reset_archive(
        &self,
        environment: &str,
        cuid: &str,
        count: u64,
    ) -> Result<(), String> {
        validate_environment(environment)?;
        validate_count(count)?;
        if cuid.trim().is_empty() {
            return Ok(());
        }
        let _transaction = self
            .transaction
            .lock()
            .map_err(|_| "counter_store_lock_poisoned")?;
        let mut data = self.read()?;
        data.reset_archive
            .entry(environment.to_owned())
            .or_default()
            .insert(cuid.to_owned(), count);
        self.write(&data)
    }

    /// Drop all counters of one environment while keeping the reset archive.
    pub fn reset_environment(&self, environment: &str) -> Result<(), String> {
        validate_environment(environment)?;
        let _transaction = self
            .transaction
            .lock()
            .map_err(|_| "counter_store_lock_poisoned")?;
        let mut data = self.read()?;
        data.environments.remove(environment);
        self.write(&data)
    }

    fn read(&self) -> Result<CounterData, String> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(CounterData::default())
            }
            Err(error) => return Err(format!("counter_store_read_failed: {error}")),
        };
        let data: CounterData = serde_json::from_reader(file)
            .map_err(|error| format!("counter_store_invalid_json: {error}"))?;
        if data.version != 1 {
            return Err(format!(
                "counter_store_unsupported_version: {}",
                data.version
            ));
        }
        for (environment, entry) in &data.environments {
            validate_environment(environment)?;
            validate_count(entry.key_press)?;
            for (cuid, count) in &entry.cumulative {
                if cuid.trim().is_empty() {
                    return Err("counter_store_invalid_account".into());
                }
                validate_count(*count)?;
            }
            if let Some(cuid) = &entry.active_cuid {
                if !entry.cumulative.contains_key(cuid) {
                    return Err("counter_store_active_account_missing".into());
                }
            }
        }
        for (environment, archive) in &data.reset_archive {
            validate_environment(environment)?;
            for count in archive.values() {
                validate_count(*count)?;
            }
        }
        Ok(data)
    }

    fn write(&self, data: &CounterData) -> Result<(), String> {
        let directory = self
            .path
            .parent()
            .ok_or("counter_store_directory_missing")?;
        create_private_directory(directory)
            .map_err(|error| format!("counter_store_directory_failed: {error}"))?;
        let temporary = directory.join(format!(
            ".counters-{}-{}.tmp",
            std::process::id(),
            WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temporary)
                .map_err(|error| format!("counter_store_write_failed: {error}"))?;
            serde_json::to_writer(&mut file, data)
                .map_err(|error| format!("counter_store_serialize_failed: {error}"))?;
            file.write_all(b"\n")
                .and_then(|_| file.sync_all())
                .map_err(|error| format!("counter_store_sync_failed: {error}"))?;
            fs::rename(&temporary, &self.path)
                .map_err(|error| format!("counter_store_replace_failed: {error}"))
        })();
        match result {
            Ok(()) => Ok(()),
            Err(original) => {
                if let Err(error) = fs::remove_file(&temporary) {
                    if error.kind() != io::ErrorKind::NotFound {
                        return Err(format!("{original}; counter_store_cleanup_failed: {error}"));
                    }
                }
                Err(original)
            }
        }
    }
}

fn validate_environment(environment: &str) -> Result<(), String> {
    if environment.trim().is_empty() {
        Err("counter_environment_required".into())
    } else {
        Ok(())
    }
}

fn validate_count(count: u64) -> Result<(), String> {
    if count > MAX_SAFE_COUNT {
        Err("counter_exceeds_javascript_safe_integer".into())
    } else {
        Ok(())
    }
}

pub(crate) fn ensure_private_directory(path: &Path) -> io::Result<()> {
    create_private_directory(path)
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    // `create` does not change the mode of an existing directory. Tauri/WebKit
    // may have created the app data directory with the login umask, so tighten
    // it explicitly for the token and counter files stored beneath it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Marker written before the app restarts for a local reset. WebKit's
/// `clear_all_browsing_data` is asynchronous and the live WebView can keep
/// writing caches, so the next process removes WebKit's files before it opens a
/// WebView instead of trusting the live clear to finish.
const PENDING_RESET_MARKER: &str = ".reset-pending";

/// Entries WebKitGTK writes directly beneath the app data directory. Only
/// WebKit-owned data belongs here: counters, logs and anything else the app
/// owns must survive a pending reset.
const WEBKIT_DATA_ENTRIES: &[&str] = &[
    "CacheStorage",
    "WebKitCache",
    "databases",
    "hsts-storage.sqlite",
    "localstorage",
    "mediakeys",
    "storage",
];

pub(crate) fn mark_reset_pending(directory: &Path) -> io::Result<()> {
    create_private_directory(directory)?;
    let path = directory.join(PENDING_RESET_MARKER);
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(b"pending\n")?;
    file.sync_all()
}

/// Remove WebKit data left behind by a local reset before any WebView exists.
/// Returns whether a reset was pending. The marker is removed last, so a
/// failure keeps the reset pending for the next startup.
pub(crate) fn complete_pending_reset(directory: &Path) -> io::Result<bool> {
    let marker = directory.join(PENDING_RESET_MARKER);
    match fs::symlink_metadata(&marker) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    for name in WEBKIT_DATA_ENTRIES {
        let path = directory.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&path)?,
            Ok(_) => fs::remove_file(&path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    fs::remove_file(&marker)?;
    Ok(true)
}

#[tauri::command]
pub fn load_key_press_count(
    store: tauri::State<'_, CounterStore>,
    environment: String,
) -> Result<u64, String> {
    store.load_key_press(&environment)
}

#[tauri::command]
pub fn save_key_press_count(
    store: tauri::State<'_, CounterStore>,
    count: u64,
    environment: String,
) -> Result<(), String> {
    store.save_key_press(&environment, count)
}

#[tauri::command]
pub fn load_cumulative_click_count(
    store: tauri::State<'_, CounterStore>,
    environment: String,
) -> Result<CumulativeClickCount, String> {
    store.load_cumulative(&environment)
}

#[tauri::command]
pub fn save_cumulative_click_count(
    store: tauri::State<'_, CounterStore>,
    count: u64,
    cuid: String,
    environment: String,
) -> Result<(), String> {
    store.save_cumulative(&environment, &cuid, count)
}

#[tauri::command]
pub fn load_reset_click_archive(
    store: tauri::State<'_, CounterStore>,
    environment: String,
    cuid: String,
) -> Result<u64, String> {
    store.load_reset_archive(&environment, &cuid)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "wuwatappo-counters-test-{}-{}",
                std::process::id(),
                WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn store(&self) -> CounterStore {
            CounterStore::new(self.0.join("counters.json"))
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn missing_store_returns_zero_with_requested_environment() {
        let directory = TestDirectory::new();
        let store = directory.store();
        assert_eq!(store.load_key_press("global-global-prod").unwrap(), 0);
        assert_eq!(
            store.load_cumulative("global-global-prod").unwrap(),
            CumulativeClickCount {
                count: 0,
                cuid: String::new(),
                environment: "global-global-prod".into()
            }
        );
    }

    #[test]
    fn counters_survive_reopen_and_force_zero() {
        let directory = TestDirectory::new();
        directory
            .store()
            .save_key_press("global-global-prod", 123)
            .unwrap();
        assert_eq!(
            directory
                .store()
                .load_key_press("global-global-prod")
                .unwrap(),
            123
        );
        directory
            .store()
            .save_key_press("global-global-prod", 0)
            .unwrap();
        assert_eq!(
            directory
                .store()
                .load_key_press("global-global-prod")
                .unwrap(),
            0
        );
    }

    #[test]
    fn environments_are_isolated_without_filename_interpolation() {
        let directory = TestDirectory::new();
        let store = directory.store();
        store.save_key_press("global-global-prod", 20).unwrap();
        store.save_key_press("cn-prod", 30).unwrap();
        store.save_key_press("../literal-environment", 40).unwrap();
        assert_eq!(store.load_key_press("global-global-prod").unwrap(), 20);
        assert_eq!(store.load_key_press("cn-prod").unwrap(), 30);
        assert_eq!(store.load_key_press("../literal-environment").unwrap(), 40);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn cumulative_counts_keep_separate_account_records() {
        let directory = TestDirectory::new();
        let store = directory.store();
        store
            .save_cumulative("global-global-prod", "account-a", 100)
            .unwrap();
        store
            .save_cumulative("global-global-prod", "account-b", 200)
            .unwrap();
        let result = store.load_cumulative("global-global-prod").unwrap();
        assert_eq!(result.cuid, "account-b");
        assert_eq!(result.count, 200);
        let data = store.read().unwrap();
        assert_eq!(
            data.environments["global-global-prod"].cumulative["account-a"],
            100
        );
        assert_eq!(store.load_cumulative("cn-prod").unwrap().count, 0);
    }

    #[test]
    fn rejects_out_of_range_counts_without_replacing_valid_data() {
        let directory = TestDirectory::new();
        let store = directory.store();
        store
            .save_key_press("global-global-prod", MAX_SAFE_COUNT)
            .unwrap();
        let original = fs::read(&store.path).unwrap();
        assert!(store
            .save_key_press("global-global-prod", MAX_SAFE_COUNT + 1)
            .is_err());
        assert!(store
            .save_cumulative("global-global-prod", "account-a", MAX_SAFE_COUNT + 1)
            .is_err());
        assert!(store
            .save_cumulative("global-global-prod", "  ", 1)
            .is_err());
        assert!(store.save_key_press("  ", 1).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), original);
    }

    #[test]
    fn malformed_store_is_not_reset_by_a_save() {
        let directory = TestDirectory::new();
        let store = directory.store();
        fs::write(&store.path, b"{broken-json").unwrap();
        assert!(store
            .load_key_press("global-global-prod")
            .unwrap_err()
            .contains("invalid_json"));
        assert!(store.save_key_press("global-global-prod", 42).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), b"{broken-json");
    }

    #[test]
    fn rejects_unknown_schema_and_inconsistent_account_records() {
        let directory = TestDirectory::new();
        let store = directory.store();
        fs::write(&store.path, r#"{"version":2,"environments":{}}"#).unwrap();
        assert!(store
            .load_key_press("env")
            .unwrap_err()
            .contains("unsupported_version"));
        fs::write(&store.path, r#"{"version":1,"environments":{"env":{"key_press":0,"active_cuid":"missing","cumulative":{}}}}"#).unwrap();
        assert!(store
            .load_cumulative("env")
            .unwrap_err()
            .contains("active_account_missing"));
    }

    #[test]
    fn persisted_values_use_a_private_atomic_file() {
        let directory = TestDirectory::new();
        let store = directory.store();
        store
            .save_cumulative("global-global-prod", "account-a", MAX_SAFE_COUNT)
            .unwrap();
        let encoded =
            serde_json::to_value(store.load_cumulative("global-global-prod").unwrap()).unwrap();
        assert_eq!(encoded["environment"], "global-global-prod");
        assert_eq!(encoded["cuid"], "account-a");
        assert_eq!(encoded["count"].as_u64(), Some(MAX_SAFE_COUNT));
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn reset_archive_survives_environment_reset() {
        let directory = TestDirectory::new();
        let store = directory.store();
        store.save_key_press("cn-cn-prod", 42).unwrap();
        store
            .save_cumulative("cn-cn-prod", "account-a", 1200)
            .unwrap();
        store
            .save_reset_archive("cn-cn-prod", "account-a", 1300)
            .unwrap();
        store.reset_environment("cn-cn-prod").unwrap();
        assert_eq!(store.load_key_press("cn-cn-prod").unwrap(), 0);
        assert_eq!(store.load_cumulative("cn-cn-prod").unwrap().count, 0);
        assert_eq!(
            store.load_reset_archive("cn-cn-prod", "account-a").unwrap(),
            1300
        );
        assert_eq!(
            store.load_reset_archive("cn-cn-prod", "account-b").unwrap(),
            0
        );
        assert_eq!(store.load_reset_archive("cn-prod", "account-a").unwrap(), 0);
    }

    #[test]
    fn reset_archive_skips_anonymous_accounts_and_rejects_bad_counts() {
        let directory = TestDirectory::new();
        let store = directory.store();
        store.save_reset_archive("cn-cn-prod", "  ", 10).unwrap();
        assert_eq!(store.load_reset_archive("cn-cn-prod", "  ").unwrap(), 0);
        assert!(store
            .save_reset_archive("cn-cn-prod", "account-a", MAX_SAFE_COUNT + 1)
            .is_err());
        assert!(store.save_reset_archive("", "account-a", 10).is_err());
    }

    #[test]
    fn legacy_store_without_reset_archive_still_loads() {
        let directory = TestDirectory::new();
        let store = directory.store();
        fs::write(
            &store.path,
            r#"{"version":1,"environments":{"env":{"key_press":7,"active_cuid":null,"cumulative":{}}}}"#,
        )
        .unwrap();
        assert_eq!(store.load_key_press("env").unwrap(), 7);
        assert_eq!(store.load_reset_archive("env", "account-a").unwrap(), 0);
    }

    #[test]
    fn pending_reset_removes_webkit_data_and_keeps_application_files() {
        let directory = TestDirectory::new();
        let root = &directory.0;
        fs::write(root.join("counters.json"), b"{}\n").unwrap();
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::write(root.join("logs").join("app.log"), b"log\n").unwrap();
        for name in [
            "CacheStorage",
            "WebKitCache",
            "databases",
            "localstorage",
            "storage",
        ] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("stale"), b"stale\n").unwrap();
        }
        fs::write(root.join("hsts-storage.sqlite"), b"stale\n").unwrap();
        fs::write(root.join("mediakeys"), b"stale\n").unwrap();

        mark_reset_pending(root).unwrap();
        assert!(root.join(PENDING_RESET_MARKER).exists());
        assert!(complete_pending_reset(root).unwrap());

        assert!(!root.join(PENDING_RESET_MARKER).exists());
        for name in [
            "CacheStorage",
            "WebKitCache",
            "databases",
            "localstorage",
            "storage",
            "hsts-storage.sqlite",
            "mediakeys",
        ] {
            assert!(!root.join(name).exists(), "{name} should be removed");
        }
        assert!(root.join("counters.json").exists());
        assert!(root.join("logs").join("app.log").exists());
        assert!(!complete_pending_reset(root).unwrap());
    }
}
