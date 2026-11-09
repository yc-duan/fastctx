//! Private writer roles cannot be obtained by business code or transfer callers.
use fastctx_storage::{
    Guard, Result,
    authorization::{Write, WritePolicy},
    guard::{Action, Change},
    rusqlite::Connection,
};
use std::{cell::Cell, marker::PhantomData, rc::Rc, sync::Arc};

#[derive(Clone, Copy)]
pub(crate) enum Role {
    Scheduling,
    Content,
    Roots,
    FormatUpgrade,
    Pending,
    Source,
    Projection,
    Supply,
    Scopes,
    Reconcile,
    Authority,
    Replica,
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

#[path = "pending_audit.rs"]
mod pending_audit;
#[path = "references.rs"]
mod references;
#[derive(Default)]
struct Rules {
    references: references::References,
    pending: pending_audit::PendingAudit,
    supply: crate::blocks::supply::audit::SupplyAudit,
}
fn permits(table: &str) -> bool {
    if !table.starts_with("sync_") || table == "sync_legacy_pins" {
        return true;
    }
    match ACTIVE.get() {
        Some(Role::Scheduling) => matches!(table, "sync_tasks" | "sync_local_head" | "sync_dependencies"),
        Some(Role::Content) => matches!(
            table,
            "sync_asset_blocks"
                | "sync_asset_representations"
                | "sync_asset_manifests"
                | "sync_asset_edges"
                | "sync_asset_access"
                | "sync_asset_pins"
                | "sync_asset_uploads"
                | "sync_asset_gc"
                | "sync_root_leases"
                | "sync_tasks"
        ),
        Some(Role::Roots) => matches!(table, "sync_root_leases" | "sync_asset_gc"),
        Some(Role::Supply) => matches!(
            table,
            "sync_supply_checks"
                | "sync_supply_work"
                | "sync_root_leases"
                | "sync_asset_gc"
                | "sync_asset_access"
                | "sync_asset_uploads"
                | "sync_tasks"
        ),
        Some(Role::FormatUpgrade) => table == "sync_format_upgrades",
        Some(Role::Scopes) => matches!(table, "sync_scopes" | "sync_scope_reconciliations" | "sync_tasks"),
        Some(Role::Reconcile) => matches!(
            table,
            "sync_scope_reconciliations"
                | "sync_root_leases"
                | "sync_asset_uploads"
                | "sync_asset_pins"
                | "sync_asset_gc"
                | "sync_source_steps"
                | "sync_source_step_targets"
                | "sync_tasks"
        ),
        // What a hub decides: scope rows and their feeds, the streams it orders or
        // accepts, the control statements it signs and the content they retain.
        Some(Role::Authority) => matches!(
            table,
            "sync_scopes"
                | "sync_scope_heads"
                | "sync_changes"
                | "sync_objects"
                | "sync_hub_control_heads"
                | "sync_hub_signing_keys"
                | "sync_epoch_descriptors"
                | "sync_source_proofs"
                | "sync_stream_heads"
                | "sync_stream_records"
                | "sync_stream_roots"
                | "sync_asset_access"
                | "sync_asset_pins"
                | "sync_asset_uploads"
                | "sync_asset_gc"
                | "sync_root_leases"
        ),
        Some(Role::Source) => matches!(
            table,
            "sync_source_step_heads"
                | "sync_source_steps"
                | "sync_source_step_targets"
                | "sync_asset_uploads"
                | "sync_asset_pins"
                | "sync_asset_gc"
                | "sync_asset_access"
                | "sync_root_leases"
                | "sync_sources"
                | "sync_source_index"
                | "sync_source_proofs"
                | "sync_stream_heads"
                | "sync_stream_records"
                | "sync_stream_roots"
                | "sync_local_head"
                | "sync_tasks"
        ),
        Some(Role::Projection) => matches!(
            table,
            "sync_timeline_heads"
                | "sync_timeline_items"
                | "sync_timeline_rebuilds"
                | "sync_asset_uploads"
                | "sync_local_head"
                | "sync_tasks"
        ),
        Some(Role::Replica) => matches!(
            table,
            "sync_replica_proofs"
                | "sync_replica_epoch_fences"
                | "sync_replica_scopes"
                | "sync_replica_pages"
                | "sync_replica_versions"
                | "sync_replica_streams"
                | "sync_replica_waiting"
                | "sync_replica_content"
                | "sync_tombstones"
                | "sync_objects"
                | "sync_object_versions"
                | "sync_asset_roots"
                | "sync_asset_access"
                | "sync_epoch_descriptors"
                | "sync_source_proofs"
                | "sync_stream_heads"
                | "sync_stream_records"
                | "sync_stream_roots"
                | "sync_local_head"
                | "sync_local_changes"
                | "sync_asset_pins"
                | "sync_asset_uploads"
                | "sync_asset_gc"
                | "sync_asset_manifests"
                | "sync_tasks"
        ),
        Some(Role::Pending) => matches!(
            table,
            "sync_pending_mutations"
                | "sync_pending_objects"
                | "sync_pending_roots"
                | "sync_local_head"
                | "sync_local_changes"
                | "sync_outbox"
                | "sync_asset_access"
                | "sync_tasks"
        ),
        None => false,
    }
}
impl WritePolicy for Rules {
    fn permits(&self, write: Write<'_>) -> bool {
        permits(write.table) && (write.table != "sync_local_head" || write.action == Action::Update)
    }
}
impl Guard for Rules {
    fn check(&mut self, change: &Change<'_>) -> Result<()> {
        if change.table() == "sync_tasks"
            && matches!(ACTIVE.get(), Some(Role::Content | Role::Pending))
            && (change.action() == Action::Delete
                || change.after(0)? != fastctx_storage::rusqlite::types::ValueRef::Text(b"sync.manifest"))
        {
            return Err(super::sql::invalid("content publication cannot change another scheduler handler"));
        }
        if change.table() == "sync_tasks" && matches!(ACTIVE.get(), Some(Role::Replica)) {
            use fastctx_storage::rusqlite::types::ValueRef;
            let allowed = change.action() != Action::Delete
                && match (change.after(0)?, change.after(3)?) {
                    (ValueRef::Text(b"sync.manifest"), _) => true,
                    (ValueRef::Text(b"sync.projection"), ValueRef::Text(key)) => key.starts_with(b"derived.timeline/"),
                    (ValueRef::Text(b"sync.transfer"), ValueRef::Text(key)) => {
                        key.starts_with(b"replica-stream/") || key.starts_with(b"replica-content/")
                    }
                    _ => false,
                };
            if !allowed {
                return Err(super::sql::invalid("a replica cannot enqueue another writer's task"));
            }
        }
        if change.table() == "sync_tasks" && matches!(ACTIVE.get(), Some(Role::Projection)) {
            use fastctx_storage::rusqlite::types::ValueRef;
            let allowed = change.action() != Action::Delete
                && (change.after(0)? == ValueRef::Text(b"sync.manifest")
                    || change.after(0)? == ValueRef::Text(b"sync.projection")
                        && matches!(change.after(3)?, ValueRef::Text(key) if key.starts_with(b"derived.timeline-index/")));
            if !allowed {
                return Err(super::sql::invalid("a projection cannot change another scheduler handler"));
            }
        }
        if change.table() == "sync_tasks"
            && matches!(ACTIVE.get(), Some(Role::Scopes | Role::Reconcile))
            && (change.action() == Action::Delete
                || change.after(0)? != fastctx_storage::rusqlite::types::ValueRef::Text(b"sync.reconcile"))
        {
            return Err(super::sql::invalid("scope maintenance cannot enqueue another task family"));
        }
        // A source commit wakes content expansion for the roots it publishes and
        // the projections it marks stale, nothing else.
        if change.table() == "sync_tasks"
            && matches!(ACTIVE.get(), Some(Role::Source))
            && (change.action() == Action::Delete
                || !matches!(
                    change.after(0)?,
                    fastctx_storage::rusqlite::types::ValueRef::Text(b"sync.manifest" | b"sync.projection")
                ))
        {
            return Err(super::sql::invalid("a source commit cannot change another scheduler handler"));
        }
        if change.table() == "sync_tasks" && matches!(ACTIVE.get(), Some(Role::Supply)) {
            let allowed = change.action() != Action::Delete
                && (change.after(0)? == fastctx_storage::rusqlite::types::ValueRef::Text(b"sync.manifest")
                    || change.after(0)? == fastctx_storage::rusqlite::types::ValueRef::Text(b"sync.gc"));
            if !allowed {
                return Err(super::sql::invalid("supply verification cannot change another scheduler handler"));
            }
        }
        if change.table() == "sync_local_head"
            && matches!(ACTIVE.get(), Some(Role::Scheduling))
            && (change.action() != Action::Update
                || change.before(0)? != change.after(0)?
                || change.before(1)? != change.after(1)?)
        {
            return Err(super::sql::invalid("the scheduler cannot advance a local change cursor"));
        }
        if permits(change.table()) {
            self.references.changed(change)?;
            self.pending.changed(change)?;
            self.supply.changed(change)
        } else {
            Err(super::sql::invalid("a sync table write bypassed its typed entry point"))
        }
    }
    fn validate_operation(&mut self, db: &Connection) -> Result<()> {
        self.references.validate(db)?;
        self.pending.validate(db)?;
        self.supply.validate(db)
    }
    fn write_policy(&self) -> Option<Arc<dyn WritePolicy>> {
        Some(Arc::new(Rules::default()))
    }
    fn ended(&mut self, _: bool) {
        self.references.clear();
        self.pending.clear();
        self.supply.clear();
    }
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
    Ok(Box::new(Rules::default()))
}
