//! The content side of a timeline installation commit. A snapshot's pages are
//! staged as an ordinary upload; once the head retains the new root in the
//! same transaction, that staging closes so its pins cannot outlive a crash.
use super::uploads;
use crate::{identity::OperationId, store::access::Custody};
use fastctx_storage::{Result, rusqlite::Connection};

pub(crate) fn close_staging(db: &Connection, custody: &Custody, operation: &OperationId) -> Result<()> {
    uploads::close(db, &custody.holding.holder, custody, operation)
}
