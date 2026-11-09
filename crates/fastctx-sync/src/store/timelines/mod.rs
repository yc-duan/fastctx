//! Every holder installs its conversations' canonical item revisions into
//! immutable TimelineV1 snapshots and a local position index. Input facts in
//! the same stream only advance the installed cursor; replicas never re-merge.
mod commit;
mod install;
mod notices;
mod pages;
mod reader;
mod state;
mod stream;

pub use notices::TimelineNotices;

use crate::{
    blocks::SqliteBlocks,
    scheduler::{Lease, Outcome},
    store::FutureResult,
};
use std::sync::Arc;
use tokio::sync::Notify;

/// Installer and readers of one node share a handle, so an installation wakes
/// the subscriptions waiting on it.
#[derive(Clone)]
pub struct SqliteTimelines {
    blocks: SqliteBlocks,
    wake: Arc<Notify>,
}
impl SqliteTimelines {
    /// Shares the block store's database, holder and content budget.
    pub fn new(blocks: SqliteBlocks) -> Self {
        Self { blocks, wake: Arc::new(Notify::new()) }
    }
    /// One bounded unit of a leased `sync.projection` task for `derived.timeline`.
    /// The caller persists the outcome with the common Scheduler.
    pub fn process(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        Box::pin(install::process(self, lease))
    }
}
