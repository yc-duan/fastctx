//! Retiring any supplied pin invalidates every older receipt for that upload.
use crate::store::sql;
use fastctx_storage::{
    Result,
    guard::{Action, Change},
    rusqlite::{Connection, params, types::ValueRef},
};
use std::collections::{BTreeMap, BTreeSet};
type Key = (String, String, String, String);
#[derive(Default)]
pub(super) struct PinAudit {
    released: BTreeSet<Key>,
    advanced: BTreeMap<Key, u64>,
}
impl PinAudit {
    pub fn changed(&mut self, change: &Change<'_>) -> Result<()> {
        if change.table() == "sync_asset_pins"
            && change.action() != Action::Insert
            && change.before(9)? != ValueRef::Text(b"released")
            && (change.action() == Action::Delete || change.after(9)? == ValueRef::Text(b"released"))
        {
            self.released.insert(key(change, true)?);
        }
        if change.table() == "sync_asset_uploads" && change.action() == Action::Update {
            let before = counter(change.before(12)?)?;
            let after = counter(change.after(12)?)?;
            if before != after {
                if before.checked_add(1) != Some(after) {
                    return Err(sql::invalid("supply revision must advance monotonically"));
                }
                self.advanced.insert(key(change, false)?, after);
            }
        }
        if self.released.len() + self.advanced.len() > 4097 {
            return Err(sql::invalid("pin invalidation exceeds its operation bound"));
        }
        Ok(())
    }
    pub fn validate(&self, db: &Connection) -> Result<()> {
        for key in &self.released {
            let expected =
                self.advanced.get(key).ok_or_else(|| sql::invalid("pin retirement omitted its supply invalidation"))?;
            let actual=db.query_row("SELECT supply_revision FROM sync_asset_uploads WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4",params![key.0,key.1,key.2,key.3],|row|sql::counter(row,0))?;
            if actual != *expected {
                return Err(sql::invalid("supply invalidation differs from its committed revision"));
            }
        }
        Ok(())
    }
    pub fn clear(&mut self) {
        self.released.clear();
        self.advanced.clear();
    }
}
fn key(change: &Change<'_>, before: bool) -> Result<Key> {
    let text = |column| {
        let value = if before { change.before(column)? } else { change.after(column)? };
        match value {
            ValueRef::Text(bytes) => String::from_utf8(bytes.to_vec()).map_err(|e| sql::invalid(e.to_string())),
            _ => Err(sql::invalid("invalid upload identity")),
        }
    };
    Ok((text(0)?, text(1)?, text(2)?, text(3)?))
}
fn counter(value: ValueRef<'_>) -> Result<u64> {
    match value {
        ValueRef::Blob(bytes) => {
            Ok(u64::from_be_bytes(bytes.try_into().map_err(|_| sql::invalid("invalid supply revision"))?))
        }
        _ => Err(sql::invalid("invalid supply revision")),
    }
}
