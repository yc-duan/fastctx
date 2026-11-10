//! Source staging probes never expose another object's or operation's private pins.
use super::{Peer, Service, sources::PeerUpload};
use crate::{
    manifest::{ContentBlock, ContentRef, ManifestReply, ManifestRoot},
    registry::REGISTRY,
    store::{Error, authority::SourceUpload},
    transfer::Message,
};
use fastctx_types::sync::source_transfer::SourceTransfer;

impl Service {
    async fn upload_request(&self, peer: &Peer, upload: &SourceTransfer) -> Result<SourceUpload, Error> {
        if upload.object.scope != upload.holding.scope
            || REGISTRY.asset(&upload.kind.kind).map_err(Error::Invalid)?.registration().schema != upload.kind.schema
        {
            return Err(Error::Invalid("source transfer coordinates differ from its registered kind or scope".into()));
        }
        Ok(SourceUpload {
            context: self.source_context(peer, &upload.holding).await?,
            holding: upload.holding.clone(),
            kind: upload.kind.kind.clone(),
            epoch: upload.epoch.clone(),
            operation: upload.operation.clone(),
        })
    }
    pub(super) async fn check_source_transfer(&self, peer: &Peer, upload: &SourceTransfer) -> Result<(), Error> {
        let request = self.upload_request(peer, upload).await?;
        let sources =
            self.sources.as_ref().ok_or_else(|| Error::Unavailable("source receiver is not installed".into()))?;
        if sources.upload_custody(&request)?.object != upload.object {
            return Err(Error::Denied("source transfer names another object".into()));
        }
        Ok(())
    }
    pub(super) async fn probe_source(
        &self,
        peer: &Peer,
        upload: SourceTransfer,
        content: ContentRef,
    ) -> Result<Message, Error> {
        let request = self.upload_request(peer, &upload).await?;
        let sources =
            self.sources.as_ref().ok_or_else(|| Error::Unavailable("source receiver is not installed".into()))?;
        let custody = sources.upload_custody(&request)?;
        if custody.object != upload.object {
            return Err(Error::Denied("source transfer names another object".into()));
        }
        let present = sources.blocks().source_content_ready(&custody, &request.operation, &content)?;
        let current = sources.upload_custody(&request)?;
        if current.holding != custody.holding || current.object != custody.object {
            return Err(Error::Revoked);
        }
        Ok(Message::SourcePresence { upload, content, present })
    }
    pub(super) async fn publish_source_manifest(
        &self,
        peer: &Peer,
        upload: SourceTransfer,
        page: ContentBlock,
    ) -> Result<Message, Error> {
        let request = self.upload_request(peer, &upload).await?;
        let sources =
            self.sources.as_ref().ok_or_else(|| Error::Unavailable("source receiver is not installed".into()))?;
        let custody = sources.upload_custody(&request)?;
        if custody.object != upload.object {
            return Err(Error::Denied("source transfer names another object".into()));
        }
        let (bytes, _permit) = sources.blocks().source_page_bytes(&custody, &request.operation, &page)?;
        let root = ManifestRoot(page.id);
        let reply = ManifestReply {
            root: root.clone(),
            page_json: String::from_utf8(bytes)
                .map_err(|_| Error::Invalid("source manifest is not UTF-8 JSON".into()))?,
        };
        self.put_source_manifest(
            peer,
            PeerUpload {
                holding: upload.holding.clone(),
                kind: upload.kind.kind.clone(),
                epoch: upload.epoch.clone(),
                operation: upload.operation.clone(),
            },
            reply,
        )
        .await?;
        self.probe_source(peer, upload, ContentRef::Manifest { root }).await
    }
}
