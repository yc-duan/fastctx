//! SQL-only root adoption follows verified object-specific content publication.
use super::{access, rows};
use crate::{
    manifest::ContentRef,
    store::{access::Custody, sql},
};
use fastctx_storage::{Result, rusqlite::Connection};

pub(crate) fn retain_supplied(db: &Connection, custody: &Custody, root: &ContentRef) -> Result<()> {
    access::require(db, custody, root)?;
    if !rows::complete(db, root)? {
        return Err(sql::invalid("a local edit cannot adopt incomplete content"));
    }
    access::grant_root(db, &custody.object, custody.kind.registration().id, root, &custody.holding)
}
