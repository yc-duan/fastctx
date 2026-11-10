//! A receiver installs remote evidence without allocating authority versions or
//! advancing a local producer. Its only connection identity is supplied by assembly.
use super::{ReplicaCommit, ReplicaContext, ReplicaSink};
use crate::{
    blocks::SqliteBlocks,
    catalog::{CatalogPage, ChangesPage},
    cursor::LocalSequence,
    identity::Producer,
    proof::{HubSigningIdentity, ProofVerifier},
    store::{Error, FutureResult, access::Principal, mutation::OperationReceipt},
};
use std::sync::Arc;

#[path = "changes.rs"]
mod changes;
#[path = "epoch_chain.rs"]
mod epoch_chain;
#[path = "follow.rs"]
mod follow;
#[path = "heads.rs"]
mod heads;
#[path = "receipts.rs"]
mod receipts;
pub use follow::{FollowScope, FollowStop, StreamNeed};
#[path = "rows.rs"]
mod rows;
#[path = "stream_rows.rs"]
mod stream_rows;
#[path = "streams.rs"]
mod streams;
pub use streams::StreamInstalled;
#[path = "content.rs"]
mod content;
#[path = "targets.rs"]
mod targets;
pub use content::{ContentPosition, ContentRequest, MissingContent};
pub use targets::StreamTarget;

/// Constructed by service assembly with its pinned hub identity and verifier.
#[derive(Clone)]
pub struct SqliteReplica {
    blocks: SqliteBlocks,
    verifier: Arc<dyn ProofVerifier>,
    hub: HubSigningIdentity,
}
impl SqliteReplica {
    pub fn new(blocks: SqliteBlocks, verifier: Arc<dyn ProofVerifier>, hub: HubSigningIdentity) -> Self {
        Self { blocks, verifier, hub }
    }
    fn connection(&self, context: &ReplicaContext) -> Result<(), Error> {
        if context.source != Producer::Hub
            || context.holding.scope.hub != self.hub.hub
            || &context.holding.holder != self.blocks.holder()
        {
            return Err(Error::Denied(
                "replica metadata requires its authenticated hub and actual local holder".into(),
            ));
        }
        Ok(())
    }
}
impl ReplicaSink for SqliteReplica {
    fn apply_changes(&self, context: ReplicaContext, page: ChangesPage) -> FutureResult<'_, ReplicaCommit> {
        Box::pin(self.changes(context, page))
    }
    fn apply_catalog(&self, _context: ReplicaContext, _page: CatalogPage) -> FutureResult<'_, ReplicaCommit> {
        Box::pin(async { Err(Error::Unavailable("catalog snapshot installation is not installed".into())) })
    }
    fn receive_operation(
        &self,
        _principal: Principal,
        _source: Producer,
        _receipt: OperationReceipt,
    ) -> FutureResult<'_, LocalSequence> {
        Box::pin(async { Err(Error::Unavailable("pending operation reconciliation is not installed".into())) })
    }
}
