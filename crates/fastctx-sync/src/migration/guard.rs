//! The carrier admits only migration writes and checks every destructive fold.

use fastctx_storage::{
    Error, Result,
    authorization::{Write, WritePolicy},
    guard::{Action, Change, Guard, MigrationPurpose},
    rusqlite::{Connection, types::ValueRef},
};
use std::{collections::BTreeSet, sync::Arc};

const TABLE: &str = "sync_legacy_pins";
const MAX_CHECKS: usize = 4096;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct RequiredSource {
    conversation: String,
    call: String,
    identity: String,
    fingerprint: String,
}

pub(super) struct PreservationGuard {
    required: BTreeSet<RequiredSource>,
    deleted: BTreeSet<String>,
    held: (i64, i64),
    operation: (i64, i64),
    transaction: (i64, i64),
}

pub(super) fn make(db: &Connection) -> Result<Box<dyn Guard>> {
    let tables = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for table in tables {
        let registered = crate::store::schema::TABLES.contains(&table.as_str())
            || crate::registry::runtime::TABLES.iter().any(|entry| entry.name == table)
            || crate::registry::legacy::ENTRIES.iter().any(|entry| {
                matches!(
                    entry.surface,
                    crate::registry::legacy::Surface::Table | crate::registry::legacy::Surface::Migration
                ) && entry.selector == table
            });
        if !registered {
            return Err(Error::new(format!("unregistered persistent table: {table}")));
        }
    }
    let held = super::totals(db)?;
    super::publish_totals(held);
    Ok(Box::new(PreservationGuard {
        required: BTreeSet::new(),
        deleted: BTreeSet::new(),
        held,
        operation: (0, 0),
        transaction: (0, 0),
    }))
}

struct Policy;
impl WritePolicy for Policy {
    fn permits(&self, write: Write<'_>) -> bool {
        write.table != TABLE
            || write.action == Action::Delete
            || super::preserving()
            || write.migration == Some(MigrationPurpose::ConversationTransfer)
            || matches!(write.accessor, Some("sync_legacy_pin_added" | "sync_legacy_pin_deleted"))
    }
}

fn text(value: ValueRef<'_>) -> Result<String> {
    match value {
        ValueRef::Text(bytes) => {
            std::str::from_utf8(bytes).map(str::to_owned).map_err(|error| Error::new(error.to_string()))
        }
        _ => Err(Error::new("a migration identity must be text")),
    }
}
fn integer(value: ValueRef<'_>) -> Result<i64> {
    match value {
        ValueRef::Integer(value) => Ok(value),
        _ => Err(Error::new("a migration total must be an integer")),
    }
}

impl PreservationGuard {
    fn require_source(&mut self, change: &Change<'_>, previous: bool) -> Result<()> {
        let value = |column| if previous { change.before(column) } else { change.after(column) };
        let source = RequiredSource {
            conversation: text(value(1)?)?,
            call: text(value(2)?)?,
            identity: text(value(3)?)?,
            fingerprint: text(value(5)?)?,
        };
        if self.required.len() == MAX_CHECKS && !self.required.contains(&source) {
            return Err(Error::new("source preservation exceeds one transaction's audit budget"));
        }
        self.required.insert(source);
        Ok(())
    }
    fn inspect(&mut self, change: &Change<'_>, transfer: bool) -> Result<()> {
        if change.table() == "run_steps" && change.action() == Action::Update && !transfer {
            if !matches!(change.before(4)?, ValueRef::Null) && change.before(4)? != change.after(4)? {
                self.require_source(change, true)?;
            }
            if matches!(change.after(4)?, ValueRef::Null) && change.before(5)? != change.after(5)? {
                self.require_source(change, false)?;
            }
        }
        if change.table() != TABLE {
            return Ok(());
        }
        let before = change.action() != Action::Insert;
        let after = change.action() != Action::Delete;
        let old_source = if before { Some(text(change.before(1)?)?) } else { None };
        let new_source = if after { Some(text(change.after(1)?)?) } else { None };
        let summary = new_source.as_deref().or(old_source.as_deref()) == Some("@totals");
        if !super::preserving() && !transfer {
            if change.action() == Action::Delete {
                let scope = text(change.before(0)?)?;
                if self.deleted.len() == MAX_CHECKS && !self.deleted.contains(&scope) {
                    return Err(Error::new("migration deletion exceeds one transaction's audit budget"));
                }
                self.deleted.insert(scope);
            } else if !(summary
                && change.action() == Action::Update
                && old_source.as_deref() == Some("@totals")
                && new_source.as_deref() == Some("@totals")
                && self.deleted.contains(&text(change.before(0)?)?))
            {
                return Err(Error::new("migration assets can only be written by their preservation entry point"));
            }
        }
        if summary {
            let old = if before { (integer(change.before(7)?)?, integer(change.before(8)?)?) } else { (0, 0) };
            let new = if after { (integer(change.after(7)?)?, integer(change.after(8)?)?) } else { (0, 0) };
            self.operation.0 += new.0 - old.0;
            self.operation.1 += new.1 - old.1;
        }
        Ok(())
    }
}
impl Guard for PreservationGuard {
    fn check(&mut self, change: &Change<'_>) -> Result<()> {
        self.inspect(change, false)
    }
    fn check_migration(&mut self, change: &Change<'_>, purpose: MigrationPurpose) -> Result<()> {
        self.inspect(change, matches!(purpose, MigrationPurpose::ConversationTransfer))
    }
    fn validate_operation(&mut self, db: &Connection) -> Result<()> {
        if !self.required.is_empty() {
            let mut pinned = db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_legacy_pins WHERE conversation=?1 AND source='run-step' AND identity=?2 AND fingerprint=?3 AND body IS NOT NULL)")?;
            for fold in &self.required {
                let key = serde_json::to_string(&(&fold.call, &fold.identity))
                    .map_err(|error| Error::new(error.to_string()))?;
                if !pinned.query_row((&fold.conversation, key, &fold.fingerprint), |row| row.get::<_, bool>(0))? {
                    return Err(Error::new(
                        "the full step source must be preserved before its body is replaced or folded",
                    ));
                }
            }
        }
        if !self.deleted.is_empty() {
            let mut deleted =
                db.prepare_cached("SELECT EXISTS(SELECT 1 FROM node_conversation_deletions WHERE name=?1)")?;
            for scope in &self.deleted {
                if !deleted.query_row([scope], |row| row.get::<_, bool>(0))? {
                    return Err(Error::new(
                        "migration assets cannot be released without a verified import or explicit deletion",
                    ));
                }
            }
        }
        Ok(())
    }
    fn write_policy(&self) -> Option<Arc<dyn WritePolicy>> {
        Some(Arc::new(Policy))
    }
    fn ended(&mut self, kept: bool) {
        if kept {
            self.transaction.0 += self.operation.0;
            self.transaction.1 += self.operation.1;
        }
        self.operation = (0, 0);
        self.required.clear();
        self.deleted.clear();
    }
    fn committed(&mut self, stood: bool) {
        if stood {
            self.held.0 += self.transaction.0;
            self.held.1 += self.transaction.1;
            super::publish_totals(self.held);
        }
        self.transaction = (0, 0);
        self.operation = (0, 0);
        self.required.clear();
        self.deleted.clear();
    }
}
