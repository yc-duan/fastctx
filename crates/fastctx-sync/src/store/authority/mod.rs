//! A hub accepts original device facts; it never re-signs them as hub facts.
mod access;
mod commit;
mod prepare;
mod receipts;
mod rejected;
mod streams;
mod waiting;

use crate::{
    blocks::SqliteBlocks,
    cursor::HoldingPeriod,
    identity::{OperationId, ProducerEpoch, StreamId},
    manifest::{ManifestReply, Pin, PublishedBlock, Representation},
    proof::{HubSigningIdentity, ProofSigner, ProofVerifier, SignedSourceBatch},
    registry::{AuthorityRule, ConflictRule, REGISTRY},
    store::{Error, FutureResult, access::AccessContext, control::AuthorityRun, source_authorize::SourceAuthorizer},
};
use std::{collections::BTreeMap, sync::Arc};

pub use fastctx_types::sync::source_acceptance::{SourceAccepted, SourceReceipt};
/// Authenticated transport supplies context; remaining coordinates grant nothing.
#[derive(Clone)]
pub struct SourceUpload {
    pub context: AccessContext,
    pub holding: HoldingPeriod,
    pub kind: String,
    pub epoch: ProducerEpoch,
    pub operation: OperationId,
}

#[derive(Clone)]
pub struct SqliteAuthority {
    blocks: SqliteBlocks,
    run: AuthorityRun,
    signer: Arc<dyn ProofSigner>,
    verifier: Arc<dyn ProofVerifier>,
    identity: HubSigningIdentity,
    authorizers: BTreeMap<&'static str, Arc<dyn SourceAuthorizer>>,
    awaiting_bytes: usize,
}
impl SqliteAuthority {
    /// `run` is shared with the directory and was persisted before construction.
    /// The awaiting limit bounds durable unaccepted metadata, not accepted assets.
    pub fn new(
        blocks: SqliteBlocks,
        run: AuthorityRun,
        signer: Arc<dyn ProofSigner>,
        verifier: Arc<dyn ProofVerifier>,
        identity: HubSigningIdentity,
        authorizers: Vec<Arc<dyn SourceAuthorizer>>,
        awaiting_bytes: usize,
    ) -> Result<Self, Error> {
        if identity.hub != *run.hub()
            || blocks.holder() != &(crate::cursor::Holder::Hub { hub: run.hub().clone() })
            || !(fastctx_types::sync::PAGE_BYTES as usize..=64 * 1024 * 1024).contains(&awaiting_bytes)
        {
            return Err(Error::Invalid("authority identity, holder or awaiting budget differs".into()));
        }
        let mut installed = BTreeMap::new();
        for authorizer in authorizers {
            let kind = REGISTRY.asset(authorizer.kind()).map_err(Error::Invalid)?.registration();
            if kind.conflict != ConflictRule::SingleWriter
                || !matches!(kind.authority, AuthorityRule::SourceDevice | AuthorityRule::LoginDevice)
                || installed.insert(kind.id, authorizer).is_some()
            {
                return Err(Error::Invalid("authority source kinds need unique device-source authorizers".into()));
            }
        }
        Ok(Self { blocks, run, signer, verifier, identity, authorizers: installed, awaiting_bytes })
    }
    pub(crate) fn blocks(&self) -> &SqliteBlocks {
        &self.blocks
    }
    pub(crate) fn upload_custody(&self, request: &SourceUpload) -> Result<crate::store::access::Custody, Error> {
        access::upload(self, request)
    }
    /// Checks source custody before a transport admits any offered body.
    pub fn check_upload(&self, request: &SourceUpload) -> Result<crate::identity::ObjectKey, Error> {
        access::upload(self, request).map(|custody| custody.object)
    }
    pub fn put_block(
        &self,
        request: SourceUpload,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> FutureResult<'_, PublishedBlock> {
        Box::pin(async move {
            let custody = access::upload(self, &request)?;
            crate::manifest::BlockStore::put(&self.blocks, custody, request.operation, representation, encoded).await
        })
    }
    pub fn put_manifest(&self, request: SourceUpload, page: ManifestReply) -> FutureResult<'_, Pin> {
        Box::pin(async move {
            let custody = access::upload(self, &request)?;
            crate::manifest::BlockStore::put_manifest(&self.blocks, custody, request.operation, page).await
        })
    }
    /// The principal is constructed by the authenticated transport, never decoded
    /// from the source message. Return only after the FULL transaction commits.
    pub fn accept(
        &self,
        context: AccessContext,
        holding: HoldingPeriod,
        signed: SignedSourceBatch,
    ) -> FutureResult<'_, SourceAccepted> {
        Box::pin(commit::accept(self, context, holding, signed))
    }
    /// Resumes a single waiting batch. Hub assembly registers this task handler.
    pub fn process(&self, lease: crate::scheduler::Lease) -> FutureResult<'_, crate::scheduler::Outcome> {
        Box::pin(waiting::process(self, lease))
    }
}

pub(super) fn encode(value: &impl serde::Serialize) -> fastctx_storage::Result<Vec<u8>> {
    super::epochs::json(value)
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> fastctx_storage::Result<T> {
    serde_json::from_slice(bytes).map_err(|e| super::sql::invalid(e.to_string()))
}
pub(super) fn namespace(scope: &crate::identity::ScopeKey, stream: &StreamId, epoch: &ProducerEpoch) -> String {
    format!("source/v1/{}/{}/{}", scope.scope.as_str(), stream.as_str(), epoch.as_str())
}
