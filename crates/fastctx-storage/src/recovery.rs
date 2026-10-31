//! Crash-resumable quarantine. A durable marker forbids opening a fresh empty
//! database even when a previous attempt moved only part of its SQLite file set.

use crate::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const FORMAT: &str = "fastctx-sqlite-quarantine-v1";
static NEXT: AtomicU64 = AtomicU64::new(0);
fn side(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}
fn marker(path: &Path) -> PathBuf {
    side(path, ".unavailable")
}
fn directory(path: &Path, id: &str) -> PathBuf {
    side(path, ".quarantine").join(id)
}

struct Intent {
    id: String,
    reason: String,
}
fn intent(path: &Path) -> Result<Option<Intent>> {
    let text = match std::fs::read_to_string(marker(path)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut lines = text.splitn(3, '\n');
    if lines.next() != Some(FORMAT) {
        return Err(Error::with_kind(ErrorKind::Invalid, "the database quarantine marker has an unknown format"));
    }
    let id = lines.next().unwrap_or_default();
    if id.is_empty() || id.len() > 96 || !id.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-') {
        return Err(Error::with_kind(ErrorKind::Invalid, "the database quarantine marker has an invalid identity"));
    }
    Ok(Some(Intent { id: id.into(), reason: lines.next().unwrap_or_default().into() }))
}

pub(super) fn check(path: &Path) -> Result<()> {
    if let Some(intent) = intent(path)? {
        return Err(Error::damaged(format!(
            "the database is unavailable; its files are preserved in {}: {}",
            directory(path, &intent.id).display(),
            intent.reason
        )));
    }
    Ok(())
}

pub(super) fn quarantine(path: &Path, cause: &Error) -> Result<PathBuf> {
    let intent = match intent(path)? {
        Some(intent) => intent,
        None => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let id = format!("{now:x}-{:x}-{:x}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
            let intent = Intent { id, reason: cause.to_string() };
            fastctx_platform::ensure_private_dir(&directory(path, &intent.id))?;
            let body = format!("{FORMAT}\n{}\n{}", intent.id, intent.reason);
            match fastctx_platform::publish_new(&marker(path), body.as_bytes(), fastctx_platform::Permissions::Private)
            {
                Ok(()) => intent,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    self::intent(path)?.ok_or_else(|| Error::new("the quarantine marker disappeared"))?
                }
                Err(error) => return Err(error.into()),
            }
        }
    };
    let target = directory(path, &intent.id);
    fastctx_platform::ensure_private_dir(&target)?;
    for suffix in ["", "-wal", "-shm"] {
        let source = side(path, suffix);
        let destination = target.join(source.file_name().ok_or_else(|| Error::new("database has no filename"))?);
        if !source.try_exists()? {
            continue;
        }
        if destination.try_exists()? {
            return Err(Error::new("both a database file and its quarantine copy exist; neither was replaced"));
        }
        std::fs::rename(&source, &destination)?;
    }
    #[cfg(unix)]
    {
        std::fs::File::open(&target)?.sync_all()?;
        std::fs::File::open(path.parent().ok_or_else(|| Error::new("database has no parent"))?)?.sync_all()?;
    }
    Ok(target)
}

/// Inspect existing bytes without opening a writable SQLite connection. In
/// particular, a broken header must not let SQLite discard its sidecar files.
pub(super) fn verify(path: &Path) -> Result<()> {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() == 0 {
        return Err(Error::damaged("an existing database is empty; its prior state cannot be established"));
    }
    let mut header = [0u8; 16];
    match file.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(Error::damaged("the database header is truncated"));
        }
        Err(error) => return Err(error.into()),
    }
    if &header != b"SQLite format 3\0" {
        return Err(Error::damaged("the database header is invalid"));
    }
    drop(file);
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    let check: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if check != "ok" {
        return Err(Error::damaged(check));
    }
    Ok(())
}
