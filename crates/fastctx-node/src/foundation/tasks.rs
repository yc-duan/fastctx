//! Which engine runs each durable task kind on this World. Kinds whose engines
//! are not here yet, such as transfers to a hub, stay parked by the worker.
use fastctx_sync::{
    blocks::SqliteBlocks,
    scheduler::TaskHandler,
    store::{sources::SqliteSources, timelines::SqliteTimelines},
};
use std::sync::Arc;

pub(super) fn handlers(
    blocks: &SqliteBlocks,
    timelines: &SqliteTimelines,
    sources: &SqliteSources,
) -> Vec<Arc<dyn TaskHandler>> {
    let mut handlers = blocks.task_handlers();
    handlers.push(Arc::new(timelines.clone()));
    handlers.push(Arc::new(sources.clone()));
    handlers
}
