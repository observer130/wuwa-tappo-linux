use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
};

const LOCK_FILE: &str = "wuwa-tappo-linux.instance.lock";

/// Holds the process-wide instance lock until dropped.
pub struct InstanceLock {
    _file: File,
}

/// Try to become the one running instance.
///
/// `Ok(Some(_))` means this process owns the lock. `Ok(None)` means another
/// process already owns it. The lock is released by the kernel when the file
/// descriptor is closed, including after a crash.
pub fn acquire() -> Result<Option<InstanceLock>, String> {
    let path = lock_path();
    acquire_at(&path).map_err(|error| format!("Could not acquire {}: {error}", path.display()))
}

fn acquire_at(path: &Path) -> io::Result<Option<InstanceLock>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(path)?;

    // WebKit spawns child processes. Do not let them inherit the lock fd, or
    // the lock would outlive the app that owns it.
    unsafe {
        libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
    }

    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(Some(InstanceLock { _file: file }));
    }

    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EWOULDBLOCK) => Ok(None),
        _ => Err(error),
    }
}

fn lock_path() -> PathBuf {
    lock_path_for(std::env::var_os("XDG_RUNTIME_DIR"))
}

fn lock_path_for(runtime_dir: Option<OsString>) -> PathBuf {
    let base = runtime_dir
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("/tmp"));
    PathBuf::from(base).join(LOCK_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_lock() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "wuwa-tappo-instance-test-{}-{}.lock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn runtime_dir_wins_and_tmp_is_the_fallback() {
        assert_eq!(
            lock_path_for(Some(OsString::from("/run/user/1000"))),
            PathBuf::from("/run/user/1000/wuwa-tappo-linux.instance.lock")
        );
        assert_eq!(
            lock_path_for(Some(OsString::new())),
            PathBuf::from("/tmp/wuwa-tappo-linux.instance.lock")
        );
        assert_eq!(
            lock_path_for(None),
            PathBuf::from("/tmp/wuwa-tappo-linux.instance.lock")
        );
    }

    #[test]
    fn second_launch_is_rejected_and_lock_is_released_on_drop() {
        let path = temporary_lock();
        let first = acquire_at(&path)
            .expect("open lock")
            .expect("first instance");
        assert!(acquire_at(&path).expect("open lock").is_none());

        drop(first);
        let second = acquire_at(&path)
            .expect("open lock")
            .expect("after release");
        drop(second);
        let _ = std::fs::remove_file(path);
    }
}
