//! Private writer roles cannot be obtained by business code or transfer callers.
use fastctx_storage::{
    Guard, Result,
    authorization::{Write, WritePolicy},
    guard::Change,
    rusqlite::Connection,
};
use std::{cell::Cell, marker::PhantomData, rc::Rc, sync::Arc};

#[derive(Clone, Copy)]
pub(crate) enum Role {
    Scheduling,
}
thread_local! { static ACTIVE: Cell<Option<Role>> = const { Cell::new(None) }; }
pub(crate) struct Boundary(Option<Role>, PhantomData<Rc<()>>);
pub(crate) fn enter_typed_write(role: Role) -> Boundary {
    Boundary(ACTIVE.replace(Some(role)), PhantomData)
}
impl Drop for Boundary {
    fn drop(&mut self) {
        ACTIVE.set(self.0);
    }
}

struct Rules;
fn permits(table: &str) -> bool {
    if !table.starts_with("sync_") || table == "sync_legacy_pins" {
        return true;
    }
    match ACTIVE.get() {
        Some(Role::Scheduling) => table == "sync_tasks",
        None => false,
    }
}
impl WritePolicy for Rules {
    fn permits(&self, write: Write<'_>) -> bool {
        permits(write.table)
    }
}
impl Guard for Rules {
    fn check(&mut self, change: &Change<'_>) -> Result<()> {
        if permits(change.table()) {
            Ok(())
        } else {
            Err(super::sql::invalid("a sync table write bypassed its typed entry point"))
        }
    }
    fn write_policy(&self) -> Option<Arc<dyn WritePolicy>> {
        Some(Arc::new(Rules))
    }
    fn ended(&mut self, _: bool) {}
    fn committed(&mut self, _: bool) {}
}

/// Services compose this with their lifecycle and legacy preservation guards.
/// Schema installation has already completed when this constructor runs.
pub fn guard(db: &Connection) -> Result<Box<dyn Guard>> {
    let mut query = db.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name GLOB 'sync_*'")?;
    let actual = query
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<fastctx_storage::rusqlite::Result<std::collections::BTreeSet<_>>>()?;
    let expected: std::collections::BTreeSet<_> = super::schema::TABLES.iter().map(|name| name.to_string()).collect();
    if actual != expected {
        return Err(super::sql::invalid("installed sync tables differ from the registry"));
    }
    Ok(Box::new(Rules))
}
