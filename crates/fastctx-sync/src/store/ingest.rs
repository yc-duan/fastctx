//! A source checkpoint, signed evidence and accepted facts commit together.

use super::{FutureResult, access::AccessContext};
use crate::{
    cursor::{ByteCount, LocalSequence, Revision, StreamCursor},
    identity::{Digest, ProducerEpoch, SourceId},
    manifest::BlobId,
};
use fastctx_types::sync::proof::{ConfirmedEpoch, SignedSourceBatch};
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
    pub expected: Option<SourceCheckpoint>,
    pub next: SourceCheckpoint,
    pub facts: SignedSourceBatch,
}
#[derive(Clone, Debug)]
pub enum IngestStatus {
    Applied { cursor: StreamCursor },
    AwaitingEpoch { proof: Digest },
}
#[derive(Clone, Debug)]
pub struct IngestReceipt {
    pub local: LocalSequence,
    pub status: IngestStatus,
}
pub trait SourceIngest: Send + Sync {
    fn checkpoint(&self, context: AccessContext, source: SourceId) -> FutureResult<'_, Option<SourceCheckpoint>>;
    fn ingest(&self, context: AccessContext, batch: IngestBatch) -> FutureResult<'_, IngestReceipt>;
    fn confirm_epoch(&self, context: AccessContext, epoch: ConfirmedEpoch) -> FutureResult<'_, ()>;
}
