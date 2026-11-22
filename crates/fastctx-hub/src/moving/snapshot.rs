//! A move copies a closed copy of each database, never a live database and
//! its log observed at different instants. The frozen round makes them
//! again.
//!
//! The hub's own databases are copied page for page: the copy is made in
//! the moments the hub is stopped for. An owner library's are written out
//! anew, only what they hold, once nothing writes them: the room their
//! deleted rows left, most of a library that was never compacted, is not
//! sent.
use fastctx_storage::rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Where the copies are made, under the hub's directory: of no listing.
const COPIES: &str = ".hub-handover";
/// Where the copy of the hub's own database was made before each had its
/// place under `COPIES`.
const FORMER: &str = ".hub-handover.sqlite3";

/// Where the copy of the database at `path` under `dir` is.
pub(super) fn path(dir: &Path, path: &str) -> PathBuf {
    let mut place = dir.join(COPIES);
    place.extend(path.split('/'));
    place
}

/// Removes the copies made under `dir`, all of them or those below `under`.
pub(super) fn clear(dir: &Path, under: Option<&str>) -> io::Result<()> {
    let place = under.map_or_else(|| dir.join(COPIES), |under| path(dir, under));
    for gone in [place, dir.join(FORMER)] {
        match fastctx_platform::disk::symlink_metadata(&gone) {
            Ok(found) if found.is_dir() => fastctx_platform::disk::remove_dir_all(&gone)?,
            Ok(_) => fastctx_platform::disk::remove_file(&gone)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Makes the closed copy of the database at `database` under `dir`:
/// `compact`, written out anew with only what it holds; else page for page.
pub(super) fn capture(dir: &Path, database: &str, compact: bool) -> io::Result<()> {
    let snapshot = path(dir, database);
    if let Some(parent) = snapshot.parent() {
        fastctx_platform::disk::create_dir_all(parent)?;
    }
    match fastctx_platform::disk::remove_file(&snapshot) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut from = dir.to_path_buf();
    from.extend(database.split('/'));
    let source = Connection::open_with_flags(&from, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(io::Error::other)?;
    source.busy_timeout(Duration::from_secs(5)).map_err(io::Error::other)?;
    if compact {
        let to = snapshot.to_str().ok_or_else(|| io::Error::other(format!("{} is not UTF-8", snapshot.display())))?;
        source.execute("VACUUM INTO ?1", [to]).map_err(io::Error::other)?;
        return Ok(());
    }
    let transaction = source.unchecked_transaction().map_err(io::Error::other)?;
    transaction
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
        .map_err(io::Error::other)?;
    let mut target = Connection::open(&snapshot).map_err(io::Error::other)?;
    target.pragma_update(None, "synchronous", "FULL").map_err(io::Error::other)?;
    {
        let backup = Backup::new(&transaction, &mut target).map_err(io::Error::other)?;
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            match backup.step(256).map_err(io::Error::other)? {
                StepResult::Done => break,
                StepResult::More => {}
                StepResult::Busy | StepResult::Locked => std::thread::sleep(Duration::from_millis(5)),
                _ => return Err(io::Error::other("unknown SQLite backup outcome")),
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("the hub database snapshot did not finish within five minutes"));
            }
        }
    }
    // Leave one independently readable file, including when the source's
    // journal mode was copied in its first page.
    target.pragma_update(None, "journal_mode", "DELETE").map_err(io::Error::other)?;
    target.close().map_err(|(_, e)| io::Error::other(e))?;
    Ok(())
}
