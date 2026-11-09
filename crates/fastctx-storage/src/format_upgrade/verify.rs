//! Integrity checking is cooperative: SQLite overflow-chain walks have no progress point.
use super::{
    FailureKind,
    snapshot::{Budget, Failure, WorkResult, read_only},
};
use crate::{Error, Result};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) fn verify(path: &Path, pages: u32, budget: &Budget) -> WorkResult<()> {
    budget.time()?;
    budget.require_running(FailureKind::Integrity)?;
    let expired = Arc::new(AtomicBool::new(false));
    let check = || -> Result<()> {
        let mut db = read_only(path)?;
        let deadline = budget.clone();
        let interrupted_by_deadline = expired.clone();
        // SQLite checkList does not invoke this callback while walking a BLOB's
        // overflow pages. The caller's bounded gate is independent (2026-11-09).
        db.progress_handler(
            1,
            Some(move || {
                if deadline.time().is_err() {
                    interrupted_by_deadline.store(true, Ordering::Release);
                    return true;
                }
                deadline.require_running(FailureKind::Integrity).is_err()
            }),
        )?;
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
    if expired.load(Ordering::Acquire) {
        return Err(Failure::new(
            FailureKind::Deadline,
            Error::new("integrity check interrupted by the backup deadline"),
        ));
    }
    budget.time()?;
    budget.require_running(FailureKind::Integrity)?;
    check.map_err(|e| Failure::new(FailureKind::Integrity, e))
}
