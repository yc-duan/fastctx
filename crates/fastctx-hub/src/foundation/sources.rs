//! Authenticated asset transport enters the hub's composed service here.
use super::Foundation;
use fastctx_sync::{
    authority::{Peer, PeerUpload},
    manifest::{ManifestReply, Pin, PublishedBlock, Representation},
    store::Error,
    transfer::Message,
};

impl Foundation {
    pub(crate) async fn answer(&self, peer: &Peer, request: Message) -> Result<Message, Error> {
        let authority = self.authority()?;
        let result = authority.service.answer(peer, request).await;
        authority.wake.notify();
        result
    }
    pub(crate) async fn put_source_block(
        &self,
        peer: &Peer,
        upload: PeerUpload,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> Result<PublishedBlock, Error> {
        let authority = self.authority()?;
        let result = authority.service.put_source_block(peer, upload, representation, encoded).await;
        authority.wake.notify();
        result
    }
    #[allow(dead_code, reason = "the asset sync transport's source manifest entry")]
    pub(crate) async fn put_source_manifest(
        &self,
        peer: &Peer,
        upload: PeerUpload,
        page: ManifestReply,
    ) -> Result<Pin, Error> {
        let authority = self.authority()?;
        let result = authority.service.put_source_manifest(peer, upload, page).await;
        authority.wake.notify();
        result
    }
}
