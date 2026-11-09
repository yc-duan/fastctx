//! The durable task kinds a block store runs, for whichever service assembles
//! it: content checks, private supply cleanup and holding reconciliation.
use super::SqliteBlocks;
use crate::{
    scheduler::{Lease, Outcome, TaskHandler, TaskKey},
    store::FutureResult,
};
use std::sync::Arc;

impl SqliteBlocks {
    /// The handlers a service's worker registers for this store's task kinds.
    pub fn task_handlers(&self) -> Vec<Arc<dyn TaskHandler>> {
        vec![Arc::new(Content(self.clone())), Arc::new(SupplyCleanup(self.clone())), Arc::new(Holdings(self.clone()))]
    }
}

/// Validation, expansion and supply checks of stored content.
struct Content(SqliteBlocks);
impl TaskHandler for Content {
    fn kind(&self) -> &'static str {
        "sync.manifest"
    }
    fn handle(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        self.0.process(lease)
    }
}

/// Private supply protection left by finished or cancelled checks.
struct SupplyCleanup(SqliteBlocks);
impl TaskHandler for SupplyCleanup {
    fn kind(&self) -> &'static str {
        "sync.gc"
    }
    fn takes(&self, key: &TaskKey) -> bool {
        key.kind == "sync.gc" && (key.key.starts_with("supply:") || key.key.starts_with("supply-cancel:"))
    }
    fn handle(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        self.0.process(lease)
    }
}

/// Scope generations retire old uploads and migrate surviving local uses.
struct Holdings(SqliteBlocks);
impl TaskHandler for Holdings {
    fn kind(&self) -> &'static str {
        "sync.reconcile"
    }
    fn takes(&self, key: &TaskKey) -> bool {
        SqliteBlocks::takes_reconciliation(key)
    }
    fn handle(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        self.0.reconcile_scope(lease)
    }
}
