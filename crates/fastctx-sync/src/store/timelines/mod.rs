//! Every holder installs its conversations' canonical item revisions into
//! immutable TimelineV1 snapshots and a local position index. Input facts in
//! the same stream only advance the installed cursor; replicas never re-merge.
mod changes;
mod commit;
mod content;
mod epochs;
mod fresh;
mod install;
mod notices;
mod pages;
mod reader;
mod rebuild;
mod standing;
mod state;
mod stream;
mod wait;

pub use notices::TimelineNotices;

use crate::{
    blocks::SqliteBlocks,
    scheduler::{Lease, Outcome},
    store::FutureResult,
};
use std::sync::Arc;
use tokio::sync::Notify;

/// Installer and readers of one node share a handle, so an installation wakes
/// the subscriptions waiting on it; so does a change to a scope's own record,
/// through the store's wake.
#[derive(Clone)]
pub struct SqliteTimelines {
    blocks: SqliteBlocks,
    wake: Arc<Notify>,
}
impl SqliteTimelines {
    /// Shares the block store's database, holder and content budget.
    pub fn new(blocks: SqliteBlocks) -> Self {
        let wake = blocks.moved().clone();
        Self { blocks, wake }
    }
    /// One bounded unit of a leased `sync.projection` task for `derived.timeline`.
    /// The caller persists the outcome with the common Scheduler.
    pub fn process(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        Box::pin(install::process(self, lease))
    }
    /// Resumes one bounded page of `derived.timeline-index/<object>` work.
    /// The common scheduler persists its outcome and retries unfinished pages.
    pub fn rebuild_index(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        Box::pin(rebuild::process(self, lease))
    }
}

impl crate::scheduler::TaskHandler for SqliteTimelines {
    fn kind(&self) -> &'static str {
        "sync.projection"
    }
    fn takes(&self, key: &crate::scheduler::TaskKey) -> bool {
        key.kind == self.kind()
            && (key.key.starts_with("derived.timeline/") || key.key.starts_with("derived.timeline-index/"))
    }
    fn handle(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        if lease.task.key.key.starts_with("derived.timeline-index/") {
            self.rebuild_index(lease)
        } else {
            self.process(lease)
        }
    }
}
