use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

use super::{Plan, files, processes};

pub(super) fn inspect(root: &Path, plan: &mut Plan) {
    let directory = root.join("update");
    let entries = super::entries(&directory, plan);
    if entries.is_empty() {
        if directory.exists()
            && files::entries(&directory).is_ok_and(|entries| entries.is_empty())
            && plan.quiet(&directory, false)
            && plan.unused(&directory, true)
        {
            plan.empty_after(&directory);
        }
        return;
    }
    // Check creators before enumerating helpers. A live TUI may spawn its
    // helper and exit between those two observations; doing them in reverse
    // would miss both processes and authorize deletion during handoff.
    for path in &entries {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if let Some(pid) = nonce_pid(name) {
            match processes::dead(pid) {
                Ok(true) => {}
                Ok(false) => {
                    plan.active(&directory, format!("transaction creator PID {pid} exists; its legacy nonce has no creation token, so the transaction is preserved"));
                    return;
                }
                Err(error) => {
                    plan.active(&directory, format!("cannot inspect transaction creator PID {pid}: {error}"));
                    return;
                }
            }
        }
    }
    if !plan.quiet(&directory, false) || !plan.unused(&directory, true) {
        return;
    }
    let lock_path = directory.join("update.lock");
    let guard = match lock_existing(&lock_path) {
        Ok(Some(file)) => Some(file),
        Ok(None) => {
            plan.active(&directory, "the legacy updater holds its lock; handshake and working files are left in place");
            return;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            plan.leave(&directory, format!("cannot acquire the existing updater lock: {error}; preserved"));
            return;
        }
    };
    if !plan.quiet(&directory, true) || !plan.unused(&directory, true) {
        return;
    }
    for path in entries {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        let regular = std::fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_file() && !crate::disk::is_link(&metadata));
        if !regular {
            plan.archive(&path, "update", "updater cache or added filesystem object retained unchanged");
            continue;
        }
        if name == "update.lock" {
            plan.archive(&path, "update", "updater lock retired after all legacy processes ended");
        } else if name.starts_with("health-") && nonce_pid(name).is_some() {
            if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() != 3) {
                plan.archive(&path, "update", "modified health acknowledgment retained unchanged");
                continue;
            }
            if let Some(snapshot) = plan.read(&path) {
                if snapshot.bytes() == Some(b"ok\n") && files::plain(&snapshot) {
                    plan.delete(snapshot, "exact legacy health acknowledgment after transaction processes ended");
                } else {
                    plan.archive(&path, "update", "modified health acknowledgment retained unchanged");
                }
            }
        } else if (name.starts_with("helper-") || name.starts_with("probe-")) && nonce_pid(name).is_some() {
            match processes::classify(&path) {
                Ok(true) => {}
                Ok(false) => {
                    plan.archive(&path, "update", "unrecognized or modified updater executable retained unchanged");
                    continue;
                }
                Err(error) => {
                    plan.active(&path, error.to_string());
                    continue;
                }
            }
            if let Some(snapshot) = plan.read(&path) {
                if snapshot.bytes().is_some_and(|bytes| processes::known_binary(&files::hash(bytes)))
                    && files::plain(&snapshot)
                {
                    plan.delete(
                        snapshot,
                        "executable matches a frozen published 0.x artifact and no update process is using it",
                    );
                } else {
                    plan.archive(&path, "update", "unrecognized or modified updater executable retained unchanged");
                }
            }
        } else {
            plan.archive(&path, "update", "updater request, handoff, cache or added content retained unchanged");
        }
    }
    plan.locks.extend(guard);
    plan.empty_after(&directory);
}

pub(super) fn nonce_pid(name: &str) -> Option<u32> {
    processes::nonce_pid(name, &["helper-", "probe-", "health-", "request-", "npm-update-", "npm-rollback-"]).or_else(
        || {
            let pid = name.strip_prefix("npm-launcher-")?.strip_suffix(".handoff")?;
            if !pid.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            pid.parse::<u32>().ok().filter(|pid| *pid != 0)
        },
    )
}

pub(super) fn lock_existing(path: &Path) -> io::Result<Option<File>> {
    files::check_path(path)?;
    lock_in_directory(path)
}

/// The caller validates and retains the parent directory's identity.
pub(super) fn lock_in_directory(path: &Path) -> io::Result<Option<File>> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    crate::disk::ordinary(&file.metadata()?)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}
