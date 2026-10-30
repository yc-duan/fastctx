//! A move copies a closed SQLite snapshot, never a live database and WAL
//! observed at different instants. The frozen round captures it again.
use fastctx_storage::rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const SNAPSHOT: &str = ".hub-handover.sqlite3";
pub(super) fn path(dir: &Path) -> PathBuf {
    dir.join(SNAPSHOT)
}

pub(super) fn capture(dir: &Path) -> io::Result<()> {
    let source = Connection::open_with_flags(dir.join(crate::store::shelf::FILE), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(io::Error::other)?;
    source.busy_timeout(Duration::from_secs(5)).map_err(io::Error::other)?;
    let transaction = source.unchecked_transaction().map_err(io::Error::other)?;
    transaction
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
        .map_err(io::Error::other)?;
    let snapshot = path(dir);
    match std::fs::remove_file(&snapshot) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
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
