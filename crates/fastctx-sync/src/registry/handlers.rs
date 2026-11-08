//! Kind-specific rules return bounded plans; the engine owns their transactions.

use crate::{
    catalog::ObjectHead,
    manifest::BlobId,
    store::{
        Error,
        mutation::{FieldEdit, ProposedObject},
    },
};

#[derive(Clone, Debug)]
pub enum ExportPlan {
    GenericRootClosure,
    HandlerPage { codec: String, state: Vec<u8> },
}
#[derive(Clone, Debug)]
pub enum DeletePlan {
    GenericTombstone,
    /// Pure metadata describing registered runtime work, never an I/O callback.
    WithRuntimeIntents {
        intents: Vec<DeleteIntent>,
    },
}
#[derive(Clone, Debug)]
pub struct DeleteIntent {
    pub kind: String,
    pub key: String,
    pub body: Vec<u8>,
}
#[derive(Clone, Debug)]
pub enum MergeOutcome {
    UnchangedBase,
    /// The engine publishes and pins these bounded bytes before its write fence.
    Merged {
        body: Vec<u8>,
    },
    Conflict,
}

/// Prepared outside the writer under a recorded revision fence. Text/document
/// limits apply here; ordinary block bytes never enter a semantic merge.
pub struct MergeBodies<'a> {
    pub base: &'a [u8],
    pub candidate: &'a [u8],
    pub current: &'a [u8],
}

/// The common engine owns pending edits, receipts, revision assignment and
/// branch retention. A handler decides semantic merges and declared fields.
pub trait KindHandler: Send + Sync {
    fn kind(&self) -> &'static str;
    fn validate(&self, schema: u16, body: &[u8]) -> Result<(), Error>;
    fn references(&self, schema: u16, body: &[u8]) -> Result<Vec<BlobId>, Error>;
    fn import_legacy(&self, shape: &str, body: &[u8]) -> Result<Vec<u8>, Error>;
    fn project_legacy(&self, head: &ObjectHead, body: &[u8], generation: u16) -> Result<Vec<u8>, Error>;
    fn project_local(&self, head: &ObjectHead, body: &[u8]) -> Result<Vec<u8>, Error>;
    fn merge(
        &self,
        proposal: &ProposedObject,
        current: &ObjectHead,
        bodies: MergeBodies<'_>,
    ) -> Result<MergeOutcome, Error>;
    /// Only the declared, permitted paths may change; unknown fields survive.
    fn patch_fields(&self, schema: u16, current: &[u8], edits: &[FieldEdit]) -> Result<Vec<u8>, Error>;
    fn export(&self, head: &ObjectHead) -> Result<ExportPlan, Error>;
    fn delete(&self, head: &ObjectHead) -> Result<DeletePlan, Error>;
}
