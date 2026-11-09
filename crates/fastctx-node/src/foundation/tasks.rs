//! Which engine runs each durable task kind on this World. Kinds whose engines
//! are not here yet, such as transfers to a hub, stay parked by the worker.
use std::sync::Arc;

use fastctx_sync::{
    blocks::SqliteBlocks,
    scheduler::{Lease, Outcome, TaskHandler, TaskKey},
    store::{FutureResult, timelines::SqliteTimelines},
};

pub(super) fn handlers(blocks: &SqliteBlocks, timelines: &SqliteTimelines) -> Vec<Arc<dyn TaskHandler>> {
    vec![Arc::new(Content(blocks.clone())), Arc::new(SupplyCleanup(blocks.clone())), Arc::new(timelines.clone())]
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
