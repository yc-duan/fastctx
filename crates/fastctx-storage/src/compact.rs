//! Giving back to the disk the room a database's deleted rows leave, which
//! the database otherwise keeps for reuse: a maintenance on a connection of
//! its own, done while no service holds the database open.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{Error, ErrorKind, Result};

/// How long a connection still closing elsewhere is waited for.
const WAIT: Duration = Duration::from_secs(10);
/// The room a compaction is given besides twice the database's size.
const SPARE: u64 = 1 << 30;

/// What compacting a database came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compacted {
    /// No database is there.
    Absent,
    /// Left as it is: its disk has `free` bytes where `needed` are asked.
    NoRoom { free: u64, needed: u64 },
    /// Compacted: the bytes of the database with its log, before and after.
    Done { before: u64, after: u64 },
}

fn with_log(path: &Path) -> u64 {
    let log = PathBuf::from(format!("{}-wal", path.display()));
    std::fs::metadata(path).map_or(0, |found| found.len()) + std::fs::metadata(log).map_or(0, |found| found.len())
}

fn fold(connection: &rusqlite::Connection) -> Result<()> {
    let busy: i64 = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if busy != 0 {
        return Err(Error::with_kind(ErrorKind::Busy, "its log could not be folded in: something else has it open"));
    }
    Ok(())
}

/// Compacts the database at `path`, only where its disk has twice its size
/// free besides a gibibyte. Another connection still open — a writer that let
/// its lock go before it ended — may have it fail as busy, the database
/// unharmed: before it is compacted, nothing is done; after, the room comes
/// back once its log is folded in, at the next run. SQLite makes its
/// temporary copy where `SQLITE_TMPDIR` says.
pub fn compact(path: &Path) -> Result<Compacted> {
    if !path.exists() {
        return Ok(Compacted::Absent);
    }
    let connection = rusqlite::Connection::open(path)?;
    connection.busy_timeout(WAIT)?;
    fold(&connection)?;
    let before = with_log(path);
    let free = fastctx_platform::space::available(path)?;
    let needed = before.saturating_mul(2).saturating_add(SPARE);
    if free < needed {
        return Ok(Compacted::NoRoom { free, needed });
    }
    connection.execute_batch("VACUUM;")?;
    fold(&connection)?;
    drop(connection);
    Ok(Compacted::Done { before, after: with_log(path) })
}
