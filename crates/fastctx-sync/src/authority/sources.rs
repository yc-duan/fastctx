//! A transport authenticates the peer; the hub chooses the source account from
//! its own scope description, never from a request's claimed principal.
use super::{Peer, Service};
use crate::{
    cursor::{Holder, HoldingPeriod},
    identity::{OperationId, ProducerEpoch},
    manifest::{ManifestReply, Pin, PublishedBlock, Representation},
    proof::SignedSourceBatch,
    scope::ScopeOwner,
    store::{
        Error, FutureResult,
        access::{AccessContext, Principal},
        authority::{SourceAccepted, SourceUpload},
    },
};

impl Service {
    pub(super) async fn source_context(&self, peer: &Peer, holding: &HoldingPeriod) -> Result<AccessContext, Error> {
        if holding.holder != (Holder::Device { device: peer.device.clone() }) {
            return Err(Error::Denied("source holding is not the authenticated peer".into()));
        }
        let descriptor = self
            .directory
            .current(holding.scope.clone())
            .await?
            .ok_or_else(|| Error::Unavailable("source scope is not described by this hub".into()))?;
        let ScopeOwner::Account { identity } = &descriptor.owner else {
            return Err(Error::Denied("a device cannot produce into a hub-owned scope".into()));
        };
        if !peer.accounts.contains(&identity.id) {
            return Err(Error::Denied("the authenticated peer does not serve the source account".into()));
        }
        Ok(AccessContext {
            principal: Principal::Account { account: identity.id.clone(), device: peer.device.clone() },
            scope: holding.scope.clone(),
            observed_acl: holding.acl_revision,
        })
    }
    pub(super) async fn accept_source(
        &self,
        peer: &Peer,
        holding: HoldingPeriod,
        batch: SignedSourceBatch,
    ) -> Result<SourceAccepted, Error> {
        let sources = self
            .sources
            .as_ref()
            .ok_or_else(|| Error::Unavailable("this hub has no source receiver installed".into()))?;
        let context = self.source_context(peer, &holding).await?;
        sources.accept(context, holding, batch).await
    }
    /// Source block transport uses the same owner lookup as source metadata.
    pub fn put_source_block<'a>(
        &'a self,
        peer: &'a Peer,
        upload: PeerUpload,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> FutureResult<'a, PublishedBlock> {
        Box::pin(async move {
            let context = self.source_context(peer, &upload.holding).await?;
            let sources = self
                .sources
                .as_ref()
                .ok_or_else(|| Error::Unavailable("this hub has no source receiver installed".into()))?;
            sources
                .put_block(
                    SourceUpload {
                        context,
                        holding: upload.holding,
                        kind: upload.kind,
                        epoch: upload.epoch,
                        operation: upload.operation,
                    },
                    representation,
                    encoded,
                )
                .await
        })
    }
    pub fn put_source_manifest<'a>(
        &'a self,
        peer: &'a Peer,
        upload: PeerUpload,
        page: ManifestReply,
    ) -> FutureResult<'a, Pin> {
        Box::pin(async move {
            let context = self.source_context(peer, &upload.holding).await?;
            let sources = self
                .sources
                .as_ref()
                .ok_or_else(|| Error::Unavailable("this hub has no source receiver installed".into()))?;
            sources
                .put_manifest(
                    SourceUpload {
                        context,
                        holding: upload.holding,
                        kind: upload.kind,
                        epoch: upload.epoch,
                        operation: upload.operation,
                    },
                    page,
                )
                .await
        })
    }
}
/// Transport coordinates carry no account or principal supplied by a client.
pub struct PeerUpload {
    pub holding: HoldingPeriod,
    pub kind: String,
    pub epoch: ProducerEpoch,
    pub operation: OperationId,
}
