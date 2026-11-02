//! The format of the hub's database. Each version that changes what the
//! database means raises it, so an older hub refuses a database it would
//! misread, as beta.13 refuses any format but 1. Before a database of an
//! older format is opened, and so upgraded, a whole copy of it is kept beside
//! it and checked; restoring that copy and the older program undoes the change.

use super::shelf::{self, FILE};
use fastctx_storage::rusqlite::{Connection, OpenFlags};
use std::{io, path::Path};

/// The format this hub writes: 2 since the hub keeps its rollout and troubles.
pub(super) const CURRENT: u64 = 2;

/// The copy kept of a database before it moves from `format`.
fn copy(dir: &Path, format: u64) -> std::path::PathBuf {
    dir.join(format!("{FILE}.format-{format}"))
}

/// Refuses a database of a newer format, and copies one of an older format
/// before anything opens it for writing.
pub(super) fn prepare(dir: &Path) -> io::Result<()> {
    let path = dir.join(FILE);
    if !fastctx_platform::disk::exists(&path)? {
        return Ok(());
    }
    let format = {
        let c = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(io::Error::other)?;
        c.busy_timeout(std::time::Duration::from_secs(5)).map_err(io::Error::other)?;
        let known: bool = c
            .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='hub_meta')", [], |row| {
                row.get(0)
            })
            .map_err(io::Error::other)?;
        if !known {
            return Ok(());
        }
        shelf::meta::<u64>(&c, "format").map_err(shelf::other)?
    };
    match format {
        None => Ok(()),
        Some(format) if format == CURRENT => Ok(()),
        Some(format) if format > CURRENT => Err(io::Error::other(format!(
            "this hub's database has format {format}, newer than this Fastctx understands ({CURRENT}); \
             install the version that wrote it or a newer one"
        ))),
        Some(format) => keep(&path, &copy(dir, format)),
    }
}

/// An online backup, checked whole; a copy that does not check is made again.
fn keep(source: &Path, target: &Path) -> io::Result<()> {
    if fastctx_platform::disk::exists(target)? {
        if checked(target)? {
            return Ok(());
        }
        fastctx_platform::disk::remove_file(target)?;
    }
    let staging = target.with_extension("partial");
    match fastctx_platform::disk::remove_file(&staging) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    {
        let c = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(io::Error::other)?;
        c.busy_timeout(std::time::Duration::from_secs(5)).map_err(io::Error::other)?;
        c.execute("VACUUM INTO ?1", [staging.to_string_lossy()]).map_err(io::Error::other)?;
    }
    if !checked(&staging)? {
        return Err(io::Error::other("the copy of the hub's database before its upgrade does not check whole"));
    }
    fastctx_platform::disk::rename(&staging, target)
}

fn checked(path: &Path) -> io::Result<bool> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(io::Error::other)?;
    let result: String = c.query_row("PRAGMA integrity_check", [], |row| row.get(0)).map_err(io::Error::other)?;
    Ok(result == "ok")
}
