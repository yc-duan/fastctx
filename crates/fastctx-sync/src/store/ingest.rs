//! Source facts, local progress, registered index effects and roots share one writer.
use super::{
    Error, FutureResult,
    access::{AccessContext, Custody},
    ingest_effects::{SourceEffects, SourceFamily, SourceIndexPage, SourceIndexRead, SourceIndexRow},
    ingest_memory::SourceMemory,
    roots::RootLease,
    source_step::{CancelProgress, SourceStep, SourceStepTarget},
};
use crate::blocks::STEP_TARGETS;
use crate::{
    catalog::SourceBatch,
    cursor::{ByteCount, LocalSequence, Revision},
    identity::{Digest, HostGeneration, OperationId, Producer, ProducerEpoch, SourceId, StreamId},
    manifest::{BlobId, ContentBlock, ContentRef, ManifestReply, ManifestRoot, Pin, PublishedBlock, Representation},
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
/// Record payload bytes one commit reads for its kind authorizers. Adapters
/// flush a step at the same size.
pub const PAYLOAD_BYTES: u64 = 4 * 1024 * 1024;

/// One destination's facts and registered effects inside a step.
#[derive(Clone)]
pub struct IngestBatch {
    pub target: SourceStepTarget,
    /// None advances local parser progress without inventing an empty source fact.
    pub facts: Option<IngestFacts>,
    pub effects: SourceEffects,
}
/// One bounded local transaction for one engine-issued step: the primary
/// source's parser checkpoint and its facts fanned out to visibility scopes.
/// Destinations carry no parser checkpoint of their own. The hub may later
/// accept each scope's signed batch independently.
#[derive(Clone)]
pub struct IngestOperation {
    step: SourceStep,
    expected: Option<SourceCheckpoint>,
    next: SourceCheckpoint,
    batches: Vec<IngestBatch>,
}
impl IngestOperation {
    pub fn new(
        step: SourceStep,
        expected: Option<SourceCheckpoint>,
        next: SourceCheckpoint,
        batches: Vec<IngestBatch>,
    ) -> Result<Self, Error> {
        if batches.is_empty() || batches.len() > STEP_TARGETS as usize {
            return Err(Error::Invalid("ingestion needs a bounded nonempty destination set".into()));
        }
        let primary = step.primary();
        let scope = &primary.custody.object.scope;
        if expected.as_ref().map(|old| old.revision) != step.expected()
            || expected.as_ref().is_some_and(|old| old.source != primary.source)
            || next.source != primary.source
            || next.revision != step.revision()
            || &next.generation != step.generation()
            || next.parser_version == 0
            || expected
                .as_ref()
                .is_some_and(|old| old.generation == next.generation && next.committed_offset < old.committed_offset)
        {
            return Err(Error::Invalid("checkpoint does not continue its step and source position".into()));
        }
        for root in expected.iter().chain(std::iter::once(&next)).filter_map(|p| p.state.as_ref()) {
            root.validate().map_err(Error::Invalid)?;
            if root.id().domain != *scope {
                return Err(Error::Invalid("checkpoint root crosses its source scope".into()));
            }
        }
        let mut keys = std::collections::BTreeSet::new();
        let mut streams = std::collections::BTreeSet::new();
        let mut payloads = 0_u64;
        let mut bytes = batches.capacity().checked_mul(size_of::<IngestBatch>()).ok_or(Error::Busy)?;
        let checkpoint = fastctx_types::sync::transport_codec::bounded_json(
            &(&expected, &next),
            fastctx_types::sync::PAGE_BYTES as usize,
        )
        .map_err(|_| Error::Invalid("checkpoint exceeds its metadata bound".into()))?;
        bytes = bytes.checked_add(checkpoint.len()).ok_or(Error::Busy)?;
        for batch in &batches {
            let bound = batch.target.step();
            if bound.key() != step.key() || bound.operation() != step.operation() {
                return Err(Error::Invalid("a destination belongs to another step".into()));
            }
            let target = batch.target.target();
            let custody = &target.custody;
            let destination = &custody.object.scope;
            let kind = custody.kind.registration();
            if destination.hub != scope.hub
                || custody.context.scope != *destination
                || custody.holding.scope != *destination
                || kind.class != DataClass::Asset
                || kind.conflict != ConflictRule::SingleWriter
                || !keys.insert((destination, &target.source))
                || !streams.insert((destination, &target.stream, &target.epoch))
            {
                return Err(Error::Invalid("ingestion source, kind or custody coordinates differ".into()));
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
                claim.validate_shape().map_err(Error::Invalid)?;
                if claim.scope != *destination
                    || claim.stream != target.stream
                    || claim.epoch != target.epoch
                    || claim.producer != target.producer
                    || claim.kind != kind.id
                    || claim.schema != kind.schema
                {
                    return Err(Error::Invalid("source batch differs from the registered target".into()));
                }
                if matches!(facts, IngestFacts::Local { .. }) && &claim.operation != step.operation() {
                    return Err(Error::Invalid("a local batch carries its step's operation".into()));
                }
                for record in &claim.records {
                    payloads = payloads.saturating_add(u64::from(record.payload.decoded_bytes));
                }
            }
        }
        if bytes > fastctx_types::sync::PAGE_BYTES as usize {
            return Err(Error::Invalid("ingestion transaction exceeds its metadata bound".into()));
        }
        if payloads > PAYLOAD_BYTES {
            return Err(Error::Invalid("ingestion reads more record payload bytes than one step allows".into()));
        }
        Ok(Self { step, expected, next, batches })
    }
    pub fn operation(&self) -> &OperationId {
        self.step.operation()
    }
    pub fn step(&self) -> &SourceStep {
        &self.step
    }
    pub fn expected(&self) -> Option<&SourceCheckpoint> {
        self.expected.as_ref()
    }
    pub fn next(&self) -> &SourceCheckpoint {
        &self.next
    }
    pub fn batches(&self) -> &[IngestBatch] {
        &self.batches
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SourceReceipt {
    pub source: SourceId,
    pub scope: crate::identity::ScopeKey,
    pub checkpoint: Revision,
    pub status: IngestStatus,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct IngestReceipt {
    pub operation: OperationId,
    pub local: LocalSequence,
    pub sources: Vec<SourceReceipt>,
    pub leases: Vec<RootLease>,
}
/// Where a committed record sits and the payload it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub record: crate::catalog::RecordRef,
    pub payload: crate::manifest::ContentBlock,
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
    /// Memory admission for the step's primary source, shared across steps and
    /// host generations. A missing kind authorizer returns Unavailable.
    fn memory(&self, step: &SourceStep) -> Result<SourceMemory, Error>;
    /// Issues the next step of `primary` at checkpoint `expected`, or reopens the
    /// identical active step after a restart. The kind needs an installed authorizer.
    fn begin_step(
        &self,
        primary: SourceTarget,
        expected: Option<Revision>,
        generation: HostGeneration,
    ) -> FutureResult<'_, SourceStep>;
    fn bind_target(&self, step: SourceStep, target: SourceTarget) -> FutureResult<'_, SourceStepTarget>;
    /// Content of a step stays private to it until its commit adopts it.
    fn put_block(
        &self,
        target: SourceStepTarget,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> FutureResult<'_, PublishedBlock>;
    /// A metadata page must follow its payload in the same step; it is validated
    /// before it is stored.
    fn put_manifest(&self, target: SourceStepTarget, page: ManifestReply) -> FutureResult<'_, Pin>;
    /// Cancels the step durably and releases up to `max` of its staged pins.
    fn cancel_step(&self, step: SourceStep, max: u32) -> FutureResult<'_, CancelProgress>;
    fn checkpoint(&self, target: SourceTarget) -> FutureResult<'_, Option<SourceCheckpoint>>;
    fn progress(&self, target: SourceTarget) -> FutureResult<'_, Option<StreamProgress>>;
    /// Only members of this source's current checkpoint closure are eligible.
    /// Block IO holds a lease and rechecks the checkpoint and revocation fences
    /// before returning. A holding transition still being applied returns Busy.
    fn read_state(&self, target: SourceTarget, request: StateRead) -> FutureResult<'_, StateReply>;
    /// Bytes of a block already granted to the destination's object, such as a
    /// descriptor an earlier step committed. Uploads of an open step stay private.
    fn read_block(&self, target: SourceTarget, block: ContentBlock) -> FutureResult<'_, Vec<u8>>;
    fn read_manifest(&self, target: SourceTarget, root: ManifestRoot) -> FutureResult<'_, ManifestReply>;
    /// Exact lookup uses the same registered family, key bound and source gate
    /// as pagination; callers need not fabricate a predecessor key.
    /// The committed record of `operation` in the destination's stream epoch,
    /// so a producer recognizes an exact replay before staging it again.
    fn recorded(&self, target: SourceTarget, operation: OperationId) -> FutureResult<'_, Option<Recorded>>;
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
