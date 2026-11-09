//! The SQLite source writer. An adapter opens an engine-issued step, uploads its
//! content through it and commits it as one transaction; it never touches SQL.
mod commit;
mod effects;
mod open;
mod reads;
mod streams;

pub use open::StreamOpening;

use super::{
    Error, FutureResult,
    access::{AccessContext, Custody},
    ingest::{
        IngestOperation, IngestReceipt, Recorded, SourceCheckpoint, SourceIngest, SourceTarget, StateRead, StateReply,
        StreamProgress, sealed,
    },
    ingest_effects::{SourceFamily, SourceIndexPage, SourceIndexRead, SourceIndexRow},
    ingest_memory::{IngestBudget, SourceMemory},
    source_authorize::SourceAuthorizer,
    source_step::{CancelProgress, SourceStep, SourceStepTarget},
};
use crate::{
    blocks::SqliteBlocks,
    cursor::Revision,
    identity::{HostGeneration, OperationId},
    manifest::{ContentBlock, ManifestReply, ManifestRoot, Pin, PublishedBlock, Representation},
    proof::{ConfirmedEpoch, ProofSigner},
    registry::{AuthorityRule, ConflictRule, DataClass, REGISTRY},
};
use std::{collections::BTreeMap, sync::Arc};

pub struct SqliteSources {
    blocks: SqliteBlocks,
    signer: Arc<dyn ProofSigner>,
    authorizers: BTreeMap<&'static str, Arc<dyn SourceAuthorizer>>,
    memory: IngestBudget,
}
impl SqliteSources {
    /// Service composition installs one authorizer per ingestible kind. A kind
    /// without one cannot open a step or commit, not even checkpoint-only.
    /// All source writers on the node receive clones of the same memory pool.
    pub fn new(
        blocks: SqliteBlocks,
        signer: Arc<dyn ProofSigner>,
        authorizers: Vec<Arc<dyn SourceAuthorizer>>,
        memory: IngestBudget,
    ) -> Result<Self, Error> {
        let mut installed = BTreeMap::new();
        for authorizer in authorizers {
            let kind = REGISTRY.asset(authorizer.kind()).map_err(Error::Invalid)?;
            let registration = kind.registration();
            if registration.class != DataClass::Asset
                || registration.conflict != ConflictRule::SingleWriter
                || !matches!(registration.authority, AuthorityRule::SourceDevice | AuthorityRule::LoginDevice)
            {
                return Err(Error::Invalid("a source authorizer needs a single-writer device source kind".into()));
            }
            if installed.insert(registration.id, authorizer).is_some() {
                return Err(Error::Invalid("one kind has two source authorizers".into()));
            }
        }
        Ok(Self { blocks, signer, authorizers: installed, memory })
    }
    fn authorizer(&self, custody: &Custody) -> Result<&Arc<dyn SourceAuthorizer>, Error> {
        self.authorizers
            .get(custody.kind.registration().id)
            .ok_or_else(|| Error::Unavailable("no source authorizer is installed for this kind".into()))
    }
}

impl sealed::SourceWriter for SqliteSources {}
impl SourceIngest for SqliteSources {
    fn memory(&self, step: &SourceStep) -> Result<SourceMemory, Error> {
        self.authorizer(&step.primary().custody)?;
        self.memory.source(step)
    }
    fn begin_step(
        &self,
        primary: SourceTarget,
        expected: Option<Revision>,
        generation: HostGeneration,
    ) -> FutureResult<'_, SourceStep> {
        Box::pin(async move {
            self.authorizer(&primary.custody)?;
            self.blocks.begin_step(primary, expected, generation).await
        })
    }
    fn bind_target(&self, step: SourceStep, target: SourceTarget) -> FutureResult<'_, SourceStepTarget> {
        Box::pin(async move {
            self.authorizer(&target.custody)?;
            self.blocks.bind_target(step, target).await
        })
    }
    fn put_block(
        &self,
        target: SourceStepTarget,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> FutureResult<'_, PublishedBlock> {
        self.blocks.put_source(target, representation, encoded)
    }
    fn put_manifest(&self, target: SourceStepTarget, page: ManifestReply) -> FutureResult<'_, Pin> {
        self.blocks.put_manifest_source(target, page)
    }
    fn cancel_step(&self, step: SourceStep, max: u32) -> FutureResult<'_, CancelProgress> {
        self.blocks.cancel_step(step, max)
    }
    fn checkpoint(&self, target: SourceTarget) -> FutureResult<'_, Option<SourceCheckpoint>> {
        Box::pin(self.read_checkpoint(target))
    }
    fn progress(&self, target: SourceTarget) -> FutureResult<'_, Option<StreamProgress>> {
        Box::pin(self.read_progress(target))
    }
    fn read_state(&self, target: SourceTarget, request: StateRead) -> FutureResult<'_, StateReply> {
        Box::pin(async move {
            self.authorizer(&target.custody)?;
            self.blocks.read_state(&target, request)
        })
    }
    fn read_block(&self, target: SourceTarget, block: ContentBlock) -> FutureResult<'_, Vec<u8>> {
        Box::pin(async move {
            self.authorizer(&target.custody)?;
            self.blocks.read_granted(&target.custody, &block).map(|(bytes, _)| bytes)
        })
    }
    fn read_manifest(&self, target: SourceTarget, root: ManifestRoot) -> FutureResult<'_, ManifestReply> {
        Box::pin(async move {
            self.authorizer(&target.custody)?;
            self.blocks.manifest_granted(&target.custody, &root)
        })
    }
    fn recorded(&self, target: SourceTarget, operation: OperationId) -> FutureResult<'_, Option<Recorded>> {
        Box::pin(self.read_recorded(target, operation))
    }
    fn get_index(
        &self,
        target: SourceTarget,
        family: SourceFamily,
        key: Vec<u8>,
    ) -> FutureResult<'_, Option<SourceIndexRow>> {
        Box::pin(self.index_row(target, family, key))
    }
    fn read_index(&self, target: SourceTarget, request: SourceIndexRead) -> FutureResult<'_, SourceIndexPage> {
        Box::pin(self.index_page(target, request))
    }
    fn ingest(&self, operation: IngestOperation) -> FutureResult<'_, IngestReceipt> {
        Box::pin(self.commit(operation))
    }
    fn confirm_epoch(&self, _context: AccessContext, _epoch: ConfirmedEpoch) -> FutureResult<'_, ()> {
        Box::pin(async { Err(Error::Unavailable("epoch confirmation arrives with the hub's acceptance path".into())) })
    }
}
