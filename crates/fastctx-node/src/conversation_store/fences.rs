//! The fences on a conversation's rows: once its deletion has begun none is
//! written or changed again, and while it is being imported none is written,
//! changed or deleted but by the import. The writer's guard keeps them on
//! every table with a conversation (`fastctx_storage::guard`); a trigger on
//! each table would make every write to it four times as dear. An import
//! sets the guard aside with the triggers (`transfer::triggers`).

use super::schema;
use fastctx_storage::{
    Error, ErrorKind, Result,
    guard::{Action, Change, Guard},
    rusqlite::{Connection, types::ValueRef},
};
use std::collections::{HashMap, HashSet};

const DELETIONS: &str = "node_conversation_deletions";
const TRANSFERS: &str = "node_conversation_transfers";

/// The guard of a node's database, as the database holds its fences.
pub(crate) fn guard(db: &Connection) -> Result<Box<dyn Guard>> {
    let mut scoped = HashMap::new();
    for table in schema::tables(db)? {
        let column = told(db, &table.name)?;
        scoped.insert(table.name, column);
    }
    let names = |sql: &str| -> Result<HashSet<String>> {
        Ok(db.prepare(sql)?.query_map([], |row| row.get(0))?.collect::<std::result::Result<_, _>>()?)
    };
    Ok(Box::new(Fences {
        scoped,
        deletion: column(db, DELETIONS, "name")?,
        transfer: column(db, TRANSFERS, "name")?,
        rejected: column(db, TRANSFERS, "rejected")?,
        deleted: names("SELECT name FROM node_conversation_deletions")?,
        importing: names("SELECT name FROM node_conversation_transfers WHERE rejected=0")?,
        operation: Vec::new(),
        transaction: Vec::new(),
    }))
}

struct Fences {
    /// Each table with a conversation, and the column that tells it.
    scoped: HashMap<String, usize>,
    deletion: usize,
    transfer: usize,
    rejected: usize,
    deleted: HashSet<String>,
    importing: HashSet<String>,
    /// How to undo what the operation in progress changed of the fences,
    /// and what the operations kept in the open transaction changed.
    operation: Vec<Undo>,
    transaction: Vec<Undo>,
}

#[derive(Clone, Copy)]
enum Fence {
    Deleted,
    Importing,
}

/// A fence as it was before a change: whether the name was in it.
struct Undo {
    fence: Fence,
    name: String,
    was: bool,
}

impl Fences {
    fn names(&mut self, fence: Fence) -> &mut HashSet<String> {
        match fence {
            Fence::Deleted => &mut self.deleted,
            Fence::Importing => &mut self.importing,
        }
    }

    fn set(&mut self, fence: Fence, name: &str, on: bool) {
        let names = self.names(fence);
        let was = names.contains(name);
        if was == on {
            return;
        }
        if on {
            names.insert(name.to_owned());
        } else {
            names.remove(name);
        }
        self.operation.push(Undo { fence, name: name.to_owned(), was });
    }

    fn undo(&mut self, changes: Vec<Undo>) {
        for Undo { fence, name, was } in changes.into_iter().rev() {
            let names = self.names(fence);
            if was {
                names.insert(name);
            } else {
                names.remove(&name);
            }
        }
    }

    /// A row of a conversation's table, refused when a fence holds it.
    fn scoped(&self, change: &Change<'_>, column: usize) -> Result<()> {
        if self.deleted.is_empty() && self.importing.is_empty() {
            return Ok(());
        }
        let action = change.action();
        let old = if action == Action::Insert { None } else { name(change.before(column)?)? };
        let new = if action == Action::Delete { None } else { name(change.after(column)?)? };
        let held = |names: &HashSet<String>| [old, new].into_iter().flatten().any(|name| names.contains(name));
        // A deleted conversation's rows may still go, by its deletion.
        if action != Action::Delete && held(&self.deleted) {
            return Err(Error::with_kind(ErrorKind::Invalid, "the conversation was deleted"));
        }
        if held(&self.importing) {
            return Err(Error::with_kind(ErrorKind::Invalid, "the conversation is being imported"));
        }
        Ok(())
    }

    /// A deletion's tombstone written, or one taken away.
    fn deletion(&mut self, change: &Change<'_>) -> Result<()> {
        let action = change.action();
        let old = if action == Action::Insert { None } else { name(change.before(self.deletion)?)?.map(str::to_owned) };
        let new = if action == Action::Delete { None } else { name(change.after(self.deletion)?)?.map(str::to_owned) };
        if let Some(old) = &old
            && new.as_ref() != Some(old)
        {
            self.set(Fence::Deleted, old, false);
        }
        if let Some(new) = new {
            self.set(Fence::Deleted, &new, true);
        }
        Ok(())
    }

    /// An import begun, rejected, taken up again or ended.
    fn transfer(&mut self, change: &Change<'_>) -> Result<()> {
        let action = change.action();
        let old = if action == Action::Insert { None } else { name(change.before(self.transfer)?)?.map(str::to_owned) };
        let new = if action == Action::Delete {
            None
        } else {
            let rejected = change.after(self.rejected)?;
            name(change.after(self.transfer)?)?
                .filter(|_| matches!(rejected, ValueRef::Integer(0) | ValueRef::Null))
                .map(str::to_owned)
        };
        if let Some(old) = old {
            self.set(Fence::Importing, &old, false);
        }
        if let Some(new) = new {
            self.set(Fence::Importing, &new, true);
        }
        Ok(())
    }
}

impl Guard for Fences {
    fn check_migration(
        &mut self,
        change: &Change<'_>,
        purpose: fastctx_storage::guard::MigrationPurpose,
    ) -> Result<()> {
        match purpose {
            fastctx_storage::guard::MigrationPurpose::ConversationTransfer
                if self.scoped.contains_key(change.table()) =>
            {
                // Transfer admission owns scoped rows, not the fence metadata (2026-11-08).
                Ok(())
            }
            fastctx_storage::guard::MigrationPurpose::ConversationTransfer => self.check(change),
        }
    }
    fn check(&mut self, change: &Change<'_>) -> Result<()> {
        if let Some(&column) = self.scoped.get(change.table()) {
            return self.scoped(change, column);
        }
        match change.table() {
            DELETIONS => self.deletion(change),
            TRANSFERS => self.transfer(change),
            _ => Ok(()),
        }
    }

    fn ended(&mut self, kept: bool) {
        let changes = std::mem::take(&mut self.operation);
        if kept {
            self.transaction.extend(changes);
        } else {
            self.undo(changes);
        }
    }

    fn committed(&mut self, stood: bool) {
        let operation = std::mem::take(&mut self.operation);
        let transaction = std::mem::take(&mut self.transaction);
        if !stood {
            self.undo(operation);
            self.undo(transaction);
        }
    }
}

/// A conversation's name as a row holds it; none for a row without one.
fn name<'a>(value: ValueRef<'a>) -> Result<Option<&'a str>> {
    match value {
        ValueRef::Null => Ok(None),
        ValueRef::Text(text) => std::str::from_utf8(text)
            .map(Some)
            .map_err(|_| Error::with_kind(ErrorKind::Invalid, "a conversation's name is not text")),
        _ => Err(Error::with_kind(ErrorKind::Invalid, "a conversation's name is not text")),
    }
}

/// Where a table tells its conversation: its own `conversation` column, or
/// its owner where that column is computed from the owner and not stored.
fn told(db: &Connection, table: &str) -> Result<usize> {
    let columns = db
        .prepare(&format!("PRAGMA table_xinfo({})", schema::quote(table)))?
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(6)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let place = |name: &str| columns.iter().find(|column| column.1 == name);
    let column = match place("conversation") {
        // Not generated, or generated and stored.
        Some((index, _, 0 | 3)) => *index,
        Some(_) => place("owner")
            .map(|column| column.0)
            .ok_or_else(|| Error::new(format!("{table} computes its conversation from no owner")))?,
        None => return Err(Error::new(format!("{table} has no conversation"))),
    };
    usize::try_from(column).map_err(|_| Error::new(format!("{table} has no conversation")))
}

fn column(db: &Connection, table: &str, name: &str) -> Result<usize> {
    let index: i64 =
        db.query_row(&format!("SELECT cid FROM pragma_table_xinfo('{table}') WHERE name=?1"), [name], |row| {
            row.get(0)
        })?;
    usize::try_from(index).map_err(|_| Error::new(format!("{table} has no {name}")))
}
