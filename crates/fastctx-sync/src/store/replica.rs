//! Normal replica installation never reallocates versions or invents pending edits.
use super::{
    FutureResult,
    access::{HoldingUse, Principal},
    mutation::OperationReceipt,
};
use crate::{
    catalog::{CatalogPage, ChangesPage},
    cursor::{HoldingPeriod, LocalSequence, PageCursor, ScopeCursor},
    identity::{ObjectKey, Producer},
};
use std::collections::BTreeMap;

mod sqlite;
pub use sqlite::{
    ContentPosition, ContentRequest, FollowScope, FollowStop, MissingContent, SqliteReplica, StreamInstalled,
    StreamNeed, StreamTarget,
};

/// Built from the authenticated connection and current local custody state,
/// never deserialized from a peer's self-reported privileges. Unlisted objects
/// request ordinary replica custody; source exceptions are checked per object.
#[derive(Clone, Debug)]
pub struct ReplicaContext {
    pub source: Producer,
    pub holding: HoldingPeriod,
    pub source_uses: BTreeMap<ObjectKey, HoldingUse>,
}
#[derive(Clone, Debug)]
pub struct ReplicaCommit {
    pub local: LocalSequence,
    /// Only present after a continuous feed or complete bounded snapshot commit.
    pub cursor: Option<ScopeCursor>,
    pub next: Option<PageCursor>,
}

pub trait ReplicaSink: Send + Sync {
    /// Verifies original proofs, visibility and custody before installing heads,
    /// version roots, local changes and snapshot progress in one transaction.
    fn apply_catalog(&self, context: ReplicaContext, page: CatalogPage) -> FutureResult<'_, ReplicaCommit>;
    /// Requires the persisted preceding cursor, including its view and prefix.
    /// Missing proofs or a discontinuity cannot advance the durable cursor.
    fn apply_changes(&self, context: ReplicaContext, page: ChangesPage) -> FutureResult<'_, ReplicaCommit>;
    /// Correlates the original local request with an authenticated authority's
    /// outcome. Acceptance roots commit before pending holds can be released;
    /// definitive rejection persists, while an unknown outcome keeps its holds.
    fn receive_operation(
        &self,
        principal: Principal,
        source: Producer,
        receipt: OperationReceipt,
    ) -> FutureResult<'_, LocalSequence>;
}
