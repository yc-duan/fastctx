//! The node-local commit sequence, advanced by every writer that changes local state.
use super::sql;
use crate::cursor::LocalSequence;
use fastctx_storage::{Result, rusqlite::Connection};

pub(crate) fn current(db: &Connection) -> Result<LocalSequence> {
    db.prepare_cached("SELECT sequence FROM sync_local_head WHERE singleton=1")?
        .query_row([], |r| sql::counter(r, 0))
        .map(LocalSequence)
        .map_err(Into::into)
}
pub(crate) fn advance(db: &Connection) -> Result<LocalSequence> {
    let next = current(db)?.0.checked_add(1).ok_or_else(|| sql::invalid("local change sequence exhausted"))?;
    db.prepare_cached("UPDATE sync_local_head SET sequence=?1 WHERE singleton=1")?
        .execute([next.to_be_bytes().as_slice()])?;
    Ok(LocalSequence(next))
}
