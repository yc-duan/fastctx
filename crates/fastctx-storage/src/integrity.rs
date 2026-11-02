//! Damage found while a database is open: by a whole-database check that runs
//! in the background, or by SQLite failing an operation as corrupt. Either
//! leaves a mark beside the file, so that the next open sets the database
//! aside and salvages what it can (`recovery`), and the process's health
//! tells of it from then on.

use crate::{Error, readings::Readings};
use std::path::PathBuf;
use std::sync::Weak;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long after opening the first check waits, so that it does not
/// compete with the work of a start; `FASTCTX_STORAGE_CHECK_SECONDS` sets it
/// for a drill.
const FIRST: Duration = Duration::from_secs(10 * 60);
/// How often the check runs again.
const AGAIN: Duration = Duration::from_secs(24 * 60 * 60);
/// How often a waiting check looks whether its database is still open.
const SLICE: Duration = Duration::from_secs(30);

pub(crate) struct Damage {
    path: PathBuf,
    name: &'static str,
    counter: &'static fastctx_platform::readings::Counter,
    noticed: AtomicBool,
}

impl Damage {
    pub fn new(path: PathBuf, readings: &Readings) -> Self {
        Self { path, name: readings.name, counter: readings.damaged, noticed: AtomicBool::new(false) }
    }

    /// Notes damage when `error` is SQLite's own word that the file is
    /// corrupt, never a service's record it could not read.
    pub fn notice(&self, error: &Error) {
        if error.is_database_damage() {
            self.found(&error.to_string());
        }
    }

    pub fn found(&self, reason: &str) {
        if self.noticed.swap(true, Ordering::Relaxed) {
            return;
        }
        self.counter.add(1);
        fastctx_platform::health::damaged(self.name);
        if let Err(error) = crate::recovery::mark_damaged(&self.path, reason) {
            eprintln!("{}: damage found ({reason}), but marking it failed: {error}", self.path.display());
        }
    }
}

/// Checks the whole database now and then, on a thread of its own, for as
/// long as it is open.
pub(crate) fn watch(damage: Weak<Damage>, path: PathBuf) -> std::io::Result<()> {
    let first = std::env::var("FASTCTX_STORAGE_CHECK_SECONDS")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(FIRST, Duration::from_secs);
    std::thread::Builder::new().name("sqlite-check".into()).spawn(move || {
        fastctx_platform::roles::set(fastctx_platform::roles::Role::Blocking);
        let mut wait = first;
        loop {
            let due = Instant::now() + wait;
            while let Some(left) = due.checked_duration_since(Instant::now()) {
                if damage.strong_count() == 0 {
                    return;
                }
                std::thread::sleep(left.min(SLICE));
            }
            let Some(open) = damage.upgrade() else { return };
            match check(&path) {
                Ok(None) => {}
                Ok(Some(reason)) => open.found(&reason),
                Err(error) => open.notice(&error),
            }
            drop(open);
            wait = AGAIN;
        }
    })?;
    Ok(())
}

/// SQLite's quick check over a connection of its own, which reads every page
/// once and holds a read snapshot while it does: what it found wrong, if
/// anything.
fn check(path: &std::path::Path) -> crate::Result<Option<String>> {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let found: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    Ok((found != "ok").then_some(found))
}
