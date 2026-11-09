//! Every holder installs its conversations' canonical item revisions into
//! immutable TimelineV1 snapshots and a local position index. Input facts in
//! the same stream only advance the installed cursor; replicas never re-merge.
mod commit;
mod evidence;
mod install;
mod pages;

use crate::{
    blocks::SqliteBlocks,
    scheduler::{Lease, Outcome},
    store::FutureResult,
};

pub struct SqliteTimelines {
    blocks: SqliteBlocks,
}
impl SqliteTimelines {
    /// Shares the block store's database, holder and content budget.
    pub fn new(blocks: SqliteBlocks) -> Self {
        Self { blocks }
    }
    /// One bounded unit of a leased `sync.projection` task for `derived.timeline`.
    /// The caller persists the outcome with the common Scheduler.
    pub fn process(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        Box::pin(install::process(self, lease))
    }
}
