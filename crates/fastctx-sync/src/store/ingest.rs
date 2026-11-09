//! Source facts, local progress, registered index effects and roots share one writer.
use super::{
    Error, FutureResult,
    access::{AccessContext, Custody},
    ingest_effects::{SourceEffects, SourceFamily, SourceIndexPage, SourceIndexRead, SourceIndexRow},
    roots::RootLease,
};
use crate::{
    catalog::SourceBatch,
    cursor::{ByteCount, LocalSequence, Revision},
    identity::{Digest, HostGeneration, OperationId, Producer, ProducerEpoch, SourceId, StreamId},
    manifest::{BlobId, ContentRef, ManifestReply, ManifestRoot},
    registry::{ConflictRule, DataClass},
};
pub use fastctx_types::sync::progress::StreamProgress;
use fastctx_types::sync::proof::{ConfirmedEpoch, SignedSourceBatch};

#[derive(Clone, Debug, serde::Serialize)]
pub struct SourceCheckpoint {
    pub source: SourceId,
    pub generation: HostGeneration,
    pub revision: Revision,
    pub committed_offset: ByteCount,
    pub parser_version: u16,
    /// Local parser state, never an asset root advertised to replicas. The writer
    /// retains the complete supplied root with this checkpoint, then releases
    /// the old checkpoint's use in the same transaction. No age-based release.
    pub state: Option<ContentRef>,
}
/// Desired source coordinates, not an authorization capability. The writer
/// matches them against authenticated dispatch/provenance and installed kinds.
#[derive(Clone)]
pub struct SourceTarget {
    pub source: SourceId,
    pub custody: Custody,
    pub stream: StreamId,
    pub epoch: ProducerEpoch,
    pub producer: Producer,
}
#[derive(Clone, Debug)]
pub enum IngestFacts {
    /// The engine signs with its actual producer key. A first unconfirmed epoch
    /// commits locally; it does not enter the shared accepted set until confirmed.
    Local { batch: SourceBatch },
    /// Original bytes are retained and forwarded, never re-signed as the relay.
    /// Missing epoch evidence may suspend this shared acceptance path.
    Proven { batch: SignedSourceBatch },
}
impl IngestFacts {
    fn claim(&self) -> Result<SourceBatch, Error> {
        match self {
            Self::Local { batch } => Ok(batch.clone()),
            Self::Proven { batch } => serde_json::from_slice(&batch.proof.data.0)
                .map_err(|_| Error::Invalid("source proof has an invalid batch shape".into())),
        }
    }
    fn bytes(&self) -> Result<usize, Error> {
        let limit = fastctx_types::sync::PAGE_BYTES as usize;
        let body = match self {
            Self::Local { batch } => {
                if batch.records.is_empty() || batch.records.len() > 256 {
                    return Err(Error::Invalid("source batch exceeds its record count".into()));
                }
                fastctx_types::sync::transport_codec::bounded_json(batch, limit)
            }
            Self::Proven { batch } => {
                if batch.proof.data.0.len() > limit {
                    return Err(Error::Invalid("source proof exceeds its metadata bound".into()));
                }
                fastctx_types::sync::transport_codec::bounded_json(batch, limit)
            }
        }
        .map_err(|_| Error::Invalid("source batch exceeds its encoded metadata bound".into()))?;
        Ok(body.len())
    }
}
#[derive(Clone)]
pub struct IngestBatch {
    pub target: SourceTarget,
    pub expected: Option<SourceCheckpoint>,
    pub next: SourceCheckpoint,
    /// None advances local parser progress without inventing an empty source fact.
    pub facts: Option<IngestFacts>,
    pub effects: SourceEffects,
}
/// One bounded local transaction, including fan-out to several visibility scopes.
/// The hub may subsequently accept each scope's signed batch independently.
#[derive(Clone)]
pub struct IngestOperation {
    operation: OperationId,
    batches: Vec<IngestBatch>,
}
impl IngestOperation {
    pub fn new(operation: OperationId, batches: Vec<IngestBatch>) -> Result<Self, Error> {
        if batches.is_empty() || batches.len() > 256 {
            return Err(Error::Invalid("ingestion needs a bounded nonempty source set".into()));
        }
        let hub = &batches[0].target.custody.object.scope.hub;
        let mut keys = std::collections::BTreeSet::new();
        let mut bytes = batches.capacity().checked_mul(size_of::<IngestBatch>()).ok_or(Error::Busy)?;
        for batch in &batches {
            let target = &batch.target;
            let custody = &target.custody;
            let scope = &custody.object.scope;
            let kind = custody.kind.registration();
            if &scope.hub != hub
                || custody.context.scope != *scope
                || custody.holding.scope != *scope
                || kind.class != DataClass::Asset
                || kind.conflict != ConflictRule::SingleWriter
                || !keys.insert((scope, &target.source))
                || batch.next.source != target.source
                || batch.expected.as_ref().is_some_and(|old| old.source != target.source)
            {
                return Err(Error::Invalid("ingestion source, kind or custody coordinates differ".into()));
            }
            if batch.next.revision.0 == 0
                || batch.next.parser_version == 0
                || batch.expected.as_ref().is_some_and(|old| {
                    batch.next.revision <= old.revision
                        || old.generation == batch.next.generation && batch.next.committed_offset < old.committed_offset
                })
            {
                return Err(Error::Invalid("checkpoint does not advance its version and source position".into()));
            }
            for root in batch.expected.iter().chain(std::iter::once(&batch.next)).filter_map(|p| p.state.as_ref()) {
                root.validate().map_err(Error::Invalid)?;
                if root.id().domain != *scope {
                    return Err(Error::Invalid("checkpoint root crosses its source scope".into()));
                }
            }
            let metadata = fastctx_types::sync::transport_codec::bounded_json(
                &(
                    &target.source,
                    &custody.object,
                    kind.id,
                    &custody.holding,
                    &custody.purpose,
                    &target.stream,
                    &target.epoch,
                    &target.producer,
                    &batch.expected,
                    &batch.next,
                ),
                fastctx_types::sync::PAGE_BYTES as usize,
            )
            .map_err(|_| Error::Invalid("source coordinates exceed their metadata bound".into()))?;
            bytes = bytes.checked_add(metadata.len()).ok_or(Error::Busy)?;
            bytes = bytes.checked_add(batch.effects.validate_bounds()?).ok_or(Error::Busy)?;
            batch.effects.validate_target(target)?;
            if let Some(facts) = &batch.facts {
                let count = facts.bytes()?;
                if count > fastctx_types::sync::PAGE_BYTES as usize {
                    return Err(Error::Invalid("source batch exceeds its metadata bound".into()));
                }
                bytes = bytes.checked_add(count).ok_or(Error::Busy)?;
                let claim = facts.claim()?;
                if claim.scope != *scope
                    || claim.stream != target.stream
                    || claim.epoch != target.epoch
                    || claim.producer != target.producer
                    || claim.kind != kind.id
                    || claim.schema != kind.schema
                    || claim.records.is_empty()
                {
                    return Err(Error::Invalid("source batch differs from the registered target".into()));
                }
            }
        }
        if bytes > fastctx_types::sync::PAGE_BYTES as usize {
            return Err(Error::Invalid("ingestion transaction exceeds its metadata bound".into()));
        }
        Ok(Self { operation, batches })
    }
    pub fn operation(&self) -> &OperationId {
        &self.operation
    }
    pub fn batches(&self) -> &[IngestBatch] {
        &self.batches
    }
}
#[derive(Clone, Debug)]
pub enum IngestStatus {
    CheckpointOnly,
    Applied {
        progress: StreamProgress,
    },
    /// Used only for forwarded evidence or handover, never to block a local
    /// producer's first epoch from ingesting, projecting and reading its facts.
    AwaitingEpoch {
        proof: Digest,
    },
}
#[derive(Clone, Debug)]
pub struct SourceReceipt {
    pub source: SourceId,
    pub scope: crate::identity::ScopeKey,
    pub checkpoint: Revision,
    pub status: IngestStatus,
}
#[derive(Clone, Debug)]
pub struct IngestReceipt {
    pub operation: OperationId,
    pub local: LocalSequence,
    pub sources: Vec<SourceReceipt>,
    pub leases: Vec<RootLease>,
}
pub(crate) mod sealed {
    pub trait SourceWriter {}
}

/// The checkpoint version fixes the private root for every page of one read.
/// Encoding selection stays inside the engine and never enters parser state.
#[derive(Clone, Debug)]
pub enum StateRead {
    Manifest { checkpoint: Revision, root: ManifestRoot },
    Block { checkpoint: Revision, id: BlobId },
}
#[derive(Clone, Debug)]
pub enum StateReply {
    Manifest(ManifestReply),
    /// Decoded, digest-checked bytes, bounded by BLOCK_BYTES.
    Bytes(Vec<u8>),
    /// The checkpoint advanced or its private state was explicitly discarded.
    Superseded {
        current: Option<Revision>,
    },
}

pub trait SourceIngest: sealed::SourceWriter + Send + Sync {
    fn checkpoint(&self, target: SourceTarget) -> FutureResult<'_, Option<SourceCheckpoint>>;
    fn progress(&self, target: SourceTarget) -> FutureResult<'_, Option<StreamProgress>>;
    /// Only members of this source's current checkpoint closure are eligible.
    /// Block IO holds a lease and rechecks the checkpoint and revocation fences
    /// before returning. A holding transition still being applied returns Busy.
    fn read_state(&self, target: SourceTarget, request: StateRead) -> FutureResult<'_, StateReply>;
    /// Exact lookup uses the same registered family, key bound and source gate
    /// as pagination; callers need not fabricate a predecessor key.
    fn get_index(
        &self,
        target: SourceTarget,
        family: SourceFamily,
        key: Vec<u8>,
    ) -> FutureResult<'_, Option<SourceIndexRow>>;
    fn read_index(&self, target: SourceTarget, request: SourceIndexRead) -> FutureResult<'_, SourceIndexPage>;
    /// Only the registered engine writer executes this plan. Checkpoint CAS,
    /// source proofs/records, families, root uses and dirty tasks commit together;
    /// no feature callback runs with a connection or opens a second transaction.
    fn ingest(&self, operation: IngestOperation) -> FutureResult<'_, IngestReceipt>;
    /// Persists confirmation and schedules bounded prefix construction over the
    /// already stored facts; it never rereads the host or reallocates identities.
    fn confirm_epoch(&self, context: AccessContext, epoch: ConfirmedEpoch) -> FutureResult<'_, ()>;
}
