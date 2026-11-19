//! Damage found while a database is open: SQLite failing an operation as
//! corrupt. It leaves a mark beside the file, so that the next open sets the
//! database aside and salvages what it can (`recovery`), and the process's
//! health tells of it from then on. No whole-database check runs in the
//! background: reading every database whole, all of them at once ten
//! minutes after a start and again each day, filled the hub's disk on a
//! small machine (2026-11-19), for little more than SQLite tells as it reads.

use crate::{Error, readings::Readings};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

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
