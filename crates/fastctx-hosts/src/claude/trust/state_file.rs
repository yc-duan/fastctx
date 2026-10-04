use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fastctx_platform::Staged;

use super::lease::Lease;

pub(super) fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // A dangling link is not a missing state file that we may replace.
            match fs::symlink_metadata(path) {
                Err(missing) if missing.kind() == io::ErrorKind::NotFound => Ok(None),
                _ => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

pub(super) fn publish(path: &Path, before: Option<&[u8]>, after: &[u8], lease: &Lease) -> io::Result<()> {
    // Claude follows the state file's symlink but locks its original name.
    // Resolve only after acquiring that lock; keep the link itself intact.
    let target = if before.is_some() { fs::canonicalize(path)? } else { path.to_path_buf() };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt as _;
        Some(if before.is_some() { fs::metadata(&target)?.permissions().mode() & 0o7777 } else { 0o600 })
    };
    #[cfg(not(unix))]
    let mode = None;
    let mut staged = Staged::create(&target, mode)?;
    staged.write_all(after)?;
    lease.check()?;
    if read(path)?.as_deref() != before || (before.is_some() && fs::canonicalize(path)? != target) {
        return Err(io::Error::other(
            "Claude global state changed without respecting its lock; trust was not published",
        ));
    }
    staged.commit().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("Claude trust publication failed (replacement may already be visible): {error}"),
        )
    })
}

pub(super) fn backup(path: &Path, directory: &Path, exists: bool) -> io::Result<()> {
    if !exists {
        return Ok(());
    }
    fs::create_dir_all(directory)?;
    let prefix = format!("{}.backup.", path.file_name().expect("state filename").to_string_lossy());
    let mut backups = list(directory, &prefix)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(io::Error::other)?.as_millis();
    let latest =
        backups.first().and_then(|name| name.strip_prefix(&prefix)).and_then(|stamp| stamp.parse::<f64>().ok());
    // Claude creates a backup at most once per 60 seconds and retains the five
    // lexically newest names. A malformed newest timestamp causes a new backup.
    if latest.is_none_or(|stamp| stamp.is_nan() || now as f64 - stamp >= 60_000.0) {
        let name = format!("{prefix}{now}");
        fs::copy(path, directory.join(&name))?;
        backups.push(name);
        backups.sort_unstable_by(|left, right| right.cmp(left));
    }
    for name in backups.into_iter().skip(5) {
        fs::remove_file(directory.join(name))?;
    }
    Ok(())
}

fn list(directory: &Path, prefix: &str) -> io::Result<Vec<String>> {
    let mut names = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| PathBuf::from(entry.file_name())))
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|name| name.to_str().filter(|name| name.starts_with(prefix)).map(str::to_owned))
        .collect::<Vec<_>>();
    names.sort_unstable_by(|left, right| right.cmp(left));
    Ok(names)
}
