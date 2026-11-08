//! Source checkpoints advance in the same transaction as their accepted facts.

use super::{CommitReceipt, FutureResult, access::AccessContext};
use crate::{
    catalog::SourceBatch,
    cursor::{ByteCount, Revision},
    identity::{ProducerEpoch, SourceId},
    manifest::BlobId,
};

#[derive(Clone, Debug)]
pub struct SourceCheckpoint {
    pub source: SourceId,
    pub generation: ProducerEpoch,
    pub revision: Revision,
    pub committed_offset: ByteCount,
    pub parser_version: u16,
    pub state: Option<BlobId>,
}

#[derive(Clone, Debug)]
pub struct IngestBatch {
    pub expected: SourceCheckpoint,
    pub next: SourceCheckpoint,
    pub facts: SourceBatch,
}

/// The adapter supplies facts, not SQL or UI patches. A rejected source batch
/// leaves both its checkpoint and its accepted sequence unchanged.
pub trait SourceIngest: Send + Sync {
    fn checkpoint(&self, context: AccessContext, source: SourceId) -> FutureResult<'_, Option<SourceCheckpoint>>;
    fn ingest(&self, context: AccessContext, batch: IngestBatch) -> FutureResult<'_, CommitReceipt>;
}
