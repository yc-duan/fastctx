//! A durable collection claim fences every new reference in the same writer.
use crate::store::sql;
use fastctx_storage::{
    Error, ErrorKind, Result,
    guard::{Action, Change},
    rusqlite::{Connection, params, types::ValueRef},
};
use std::collections::BTreeSet;

#[derive(Default)]
pub(super) struct References(BTreeSet<(String, String, String, String)>);
impl References {
    pub(super) fn changed(&mut self, change: &Change<'_>) -> Result<()> {
        if change.action() == Action::Delete {
            return Ok(());
        }
        let columns = match change.table() {
            "sync_asset_roots" => [0, 1, 5, 6],
            "sync_asset_pins" if change.after(9)? != ValueRef::Text(b"released") => [0, 1, 5, 6],
            "sync_asset_edges" => [0, 1, 3, 4],
            "sync_pending_roots" => [0, 3, 6, 7],
            "sync_root_leases" if change.after(14)? == ValueRef::Integer(0) => [0, 1, 5, 6],
            _ => return Ok(()),
        };
        let text = |column| match change.after(column)? {
            ValueRef::Text(bytes) => {
                std::str::from_utf8(bytes).map(str::to_owned).map_err(|e| sql::invalid(e.to_string()))
            }
            _ => Err(sql::invalid("content reference key is not text")),
        };
        self.0.insert((text(columns[0])?, text(columns[1])?, text(columns[2])?, text(columns[3])?));
        // One maximum-sized manifest plus its own pin must fit.
        if self.0.len() > 4097 {
            return Err(sql::invalid("operation exceeds the content-reference validation budget"));
        }
        Ok(())
    }
    pub(super) fn validate(&self, db: &Connection) -> Result<()> {
        let mut query = db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_asset_gc WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind=?4 AND claimed=1)")?;
        for (hub, scope, hash, form) in &self.0 {
            if query.query_row(params![hub, scope, hash, form], |row| row.get::<_, bool>(0))? {
                return Err(Error::with_kind(ErrorKind::Busy, "a referenced content generation is being collected"));
            }
        }
        Ok(())
    }
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}
