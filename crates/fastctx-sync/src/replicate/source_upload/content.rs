//! Children are qualified before the parent manifest is explicitly registered.
use super::*;
use crate::{
    cursor::{DurableAck, Holder},
    manifest::{ContentBlock, ContentRef, Encoding},
    replicate::BlockOffer,
    store::sources::uploads::UploadNode,
    transfer::Message,
};
use fastctx_types::sync::{session::UploadCause, source_transfer::SourceTransfer};

impl SourceUploader {
    async fn present(
        &self,
        upstream: &dyn Upstream,
        state: &SourceUploadState,
        upload: &SourceTransfer,
        content: &ContentRef,
    ) -> Result<bool, Error> {
        self.store.check(state).await?;
        let reply = tokio::time::timeout(
            self.timeout,
            upstream.exchange(Message::ProbeSource { upload: upload.clone(), content: content.clone() }),
        )
        .await
        .map_err(|_| Error::Unavailable("source presence probe timed out".into()))??;
        self.store.check(state).await?;
        match reply.value() {
            Message::SourcePresence { upload: echoed, content: root, present }
                if echoed == upload && root == content =>
            {
                Ok(*present)
            }
            Message::Revoked { holding } if holding == &state.holding => Err(Error::Revoked),
            _ => Err(Error::Invalid("source presence reply changed its operation or content".into())),
        }
    }
    async fn offer(
        &self,
        upstream: &dyn Upstream,
        state: &SourceUploadState,
        upload: &SourceTransfer,
        block: &ContentBlock,
        bytes: Vec<u8>,
    ) -> Result<(), Error> {
        self.store.check(state).await?;
        let encoded = crate::blocks::codec::encode(state.key.scope.clone(), bytes, Encoding::Raw)?;
        if &encoded.representation.content != block {
            return Err(Error::Corrupt("source block bytes differ from their durable reference".into()));
        }
        let ack = tokio::time::timeout(
            self.timeout,
            upstream.offer(BlockOffer {
                holding: upload.holding.clone(),
                object: upload.object.clone(),
                kind: upload.kind.clone(),
                operation: upload.operation.clone(),
                cause: UploadCause::Source { epoch: upload.epoch.clone() },
                representation: encoded.representation,
                encoded: encoded.bytes,
            }),
        )
        .await
        .map_err(|_| Error::Unavailable("source block upload timed out".into()))??;
        self.store.check(state).await?;
        if !matches!(&ack,DurableAck::Block{holding,object,kind,schema,content} if holding.scope==upload.holding.scope&&holding.acl_revision==upload.holding.acl_revision&&holding.publication_epoch.0>0&&holding.holder==(Holder::Hub{hub:upload.holding.scope.hub.clone()})&&object==&upload.object&&kind==&upload.kind.kind&&*schema==upload.kind.schema&&content==block)
        {
            return Err(Error::Invalid("source block acknowledgement changed its hub, object or content".into()));
        }
        Ok(())
    }
    pub(super) async fn content(
        &self,
        upstream: &dyn Upstream,
        state: &SourceUploadState,
        upload: &SourceTransfer,
        node: UploadNode,
    ) -> Result<(), Error> {
        if let ContentRef::Manifest { root } = &node.root
            && !node.expanded
        {
            let (page, _permit) = self.store.page(state, root).await?;
            return self.store.expand(state, node, page).await;
        }
        if !self.present(upstream, state, upload, &node.root).await? {
            match &node.root {
                ContentRef::Single { block, decoded_bytes } => {
                    let content = ContentBlock { id: block.clone(), decoded_bytes: *decoded_bytes };
                    let (bytes, _permit) = self.store.block(state, &content).await?;
                    self.offer(upstream, state, upload, &content, bytes).await?;
                }
                ContentRef::Manifest { root } => {
                    let (page, _permit) = self.store.page(state, root).await?;
                    let content = ContentBlock {
                        id: root.0.clone(),
                        decoded_bytes: page
                            .page_json
                            .len()
                            .try_into()
                            .map_err(|_| Error::Corrupt("source page is too large".into()))?,
                    };
                    if !self.present(upstream, state, upload, &ContentRef::from(content.clone())).await? {
                        self.offer(upstream, state, upload, &content, page.page_json.into_bytes()).await?;
                    }
                    self.store.check(state).await?;
                    let reply = tokio::time::timeout(
                        self.timeout,
                        upstream.exchange(Message::PublishSourceManifest { upload: upload.clone(), page: content }),
                    )
                    .await
                    .map_err(|_| Error::Unavailable("source manifest registration timed out".into()))??;
                    self.store.check(state).await?;
                    match reply.value() {
                        Message::SourcePresence { upload: echoed, content, present }
                            if echoed == upload && content == &node.root =>
                        {
                            if !present {
                                return Err(Error::Unavailable(
                                    "source manifest is awaiting validation or descendants".into(),
                                ));
                            }
                        }
                        _ => return Err(Error::Invalid("source manifest reply changed its operation or root".into())),
                    }
                }
            }
        }
        self.store.node_done(state, node).await
    }
}
