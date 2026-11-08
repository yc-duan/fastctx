//! Verified content and its encoded representations have separate lifetimes.
use crate::{
    cursor::{HoldingPeriod, Revision},
    identity::{Digest, OperationId},
    store::{FutureResult, access::Custody},
};
pub use fastctx_types::sync::manifest::*;
#[derive(Clone, Debug)]
pub struct Pin {
    pub operation: OperationId,
    pub root: BlobId,
    pub generation: Revision,
    pub holding: HoldingPeriod,
}
#[derive(Clone, Debug)]
pub struct PublishedBlock {
    pub content: ContentBlock,
    pub representation: Digest,
    pub pin: Pin,
}
pub trait BlockStore: Send + Sync {
    fn put(
        &self,
        custody: Custody,
        operation: OperationId,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> FutureResult<'_, PublishedBlock>;
    fn descriptor(&self, custody: Custody, id: BlobId) -> FutureResult<'_, Descriptor>;
    fn manifest(&self, custody: Custody, root: BlobId) -> FutureResult<'_, ManifestReply>;
    fn read(&self, custody: Custody, id: BlobId, representation: Digest) -> FutureResult<'_, Vec<u8>>;
    fn release(&self, custody: Custody, pin: Pin) -> FutureResult<'_, ()>;
    fn collect(&self, max_blocks: u32) -> FutureResult<'_, u32>;
}
