//! A single owned read transaction spans every Online Backup step.
use super::{Event, FailureKind, Limits, Observer};
use crate::{
    Error, Result,
    rusqlite::{
        Connection, OpenFlags,
        backup::{Backup, StepResult},
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) struct Failure {
    pub kind: FailureKind,
    pub error: Error,
}
impl Failure {
    pub fn new(kind: FailureKind, error: impl Into<Error>) -> Self {
        Self { kind, error: error.into() }
    }
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::new(FailureKind::Copy, error)
    }
}
impl From<crate::rusqlite::Error> for Failure {
    fn from(error: crate::rusqlite::Error) -> Self {
        Self::new(FailureKind::Copy, error)
    }
}
impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::new(FailureKind::Copy, error)
    }
}
pub(super) type WorkResult<T> = std::result::Result<T, Failure>;
#[derive(Clone)]
pub(super) struct Budget {
    began: Instant,
    limits: Limits,
    wal: PathBuf,
    initial_wal: u64,
}
impl Budget {
    pub fn new(database: &Path, limits: Limits) -> Result<Self> {
        let mut wal = database.as_os_str().to_owned();
        wal.push("-wal");
        let wal = PathBuf::from(wal);
        Ok(Self { began: Instant::now(), initial_wal: length(&wal)?, wal, limits })
    }
    pub fn time(&self) -> WorkResult<()> {
        if self.began.elapsed() >= self.limits.elapsed {
            return Err(Failure::new(FailureKind::Deadline, Error::new("backup exceeded its elapsed-time limit")));
        }
        Ok(())
    }
    pub fn copying(&self) -> WorkResult<()> {
        self.time()?;
        let bytes = length(&self.wal).map_err(|e| Failure::new(FailureKind::Copy, e))?;
        if bytes.saturating_sub(self.initial_wal) > self.limits.wal_growth_bytes {
            return Err(Failure::new(FailureKind::WalLimit, Error::new("backup exceeded its WAL-growth limit")));
        }
        Ok(())
    }
    pub fn space(&self, directory: &Path, logical_bytes: u64) -> WorkResult<()> {
        let required = logical_bytes
            .checked_add(logical_bytes.div_ceil(5))
            .and_then(|n| n.checked_add(self.limits.reserve_bytes))
            .ok_or_else(|| Failure::new(FailureKind::Space, Error::new("backup space requirement overflow")))?;
        let available =
            fastctx_platform::space::available(directory).map_err(|e| Failure::new(FailureKind::Space, e))?;
        if available < required {
            return Err(Failure::new(
                FailureKind::Space,
                Error::new(format!("backup needs {required} free bytes; only {available} available")),
            ));
        }
        self.time()
    }
}
fn length(path: &Path) -> Result<u64> {
    match fs::metadata(path) {
        Ok(meta) => Ok(meta.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e.into()),
    }
}
pub(super) struct Snapshot {
    connection: Connection,
    pub logical_bytes: u64,
}
impl Snapshot {
    pub fn pin(path: &Path) -> Result<Self> {
        let connection = read_only(path)?;
        connection.execute_batch("BEGIN")?;
        connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get::<_, i64>(0))?;
        let pages: u32 = connection.pragma_query_value(None, "page_count", |row| row.get(0))?;
        let page_size: u32 = connection.pragma_query_value(None, "page_size", |row| row.get(0))?;
        let logical_bytes = u64::from(pages) * u64::from(page_size);
        Ok(Self { connection, logical_bytes })
    }
    pub fn copy(self, path: &Path, budget: &Budget, observe: &Observer) -> WorkResult<u32> {
        let copy = || -> WorkResult<u32> {
            let mut target =
                Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
            target.busy_timeout(Duration::from_millis(25))?;
            target.pragma_update(None, "synchronous", "FULL")?;
            target.pragma_update(None, "cache_size", -2048)?;
            let pages;
            {
                let backup = Backup::new(&self.connection, &mut target)?;
                loop {
                    budget.copying()?;
                    let result = backup.step(64)?;
                    let progress = backup.progress();
                    observe(Event::Progress { remaining_pages: progress.remaining, total_pages: progress.pagecount });
                    match result {
                        StepResult::Done => {
                            pages = u32::try_from(progress.pagecount)
                                .map_err(|_| Error::new("invalid copied page count"))?;
                            break;
                        }
                        StepResult::More => std::thread::yield_now(),
                        StepResult::Busy | StepResult::Locked => std::thread::sleep(Duration::from_millis(5)),
                        _ => return Err(Failure::new(FailureKind::Copy, Error::new("unknown SQLite backup result"))),
                    }
                }
            }
            target.pragma_update(None, "journal_mode", "DELETE")?;
            target.close().map_err(|(_, error)| error)?;
            fastctx_platform::atomic::sync_existing(path)?;
            budget.copying()?;
            Ok(pages)
        }();
        // Drop closes the dedicated connection and releases the pinned WAL before
        // integrity checking the destination, even if copying or flushing failed.
        drop(self);
        copy
    }
}
fn read_only(path: &Path) -> Result<Connection> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    db.busy_timeout(Duration::from_millis(25))?;
    db.pragma_update(None, "cache_size", -2048)?;
    Ok(db)
}
pub(super) fn verify(path: &Path, pages: u32, budget: &Budget) -> WorkResult<()> {
    budget.time()?;
    let check = || -> Result<()> {
        let mut db = read_only(path)?;
        let deadline = budget.clone();
        // Large legacy values produce few integrity-progress calls across many
        // pages. Check every SQLite progress point, not every 4096th one.
        db.progress_handler(1, Some(move || deadline.time().is_err()))?;
        let actual: u32 = db.pragma_query_value(None, "page_count", |row| row.get(0))?;
        if actual != pages {
            return Err(Error::new("backup page count differs from its durable copy receipt"));
        }
        let mut query = db.prepare("PRAGMA integrity_check")?;
        let mut rows = query.query([])?;
        let first =
            rows.next()?.ok_or_else(|| Error::new("integrity check returned no result"))?.get::<_, String>(0)?;
        if first != "ok" || rows.next()?.is_some() {
            return Err(Error::new(format!("backup integrity check failed: {first}")));
        }
        Ok(())
    }();
    budget.time()?;
    check.map_err(|e| Failure::new(FailureKind::Integrity, e))
}
