//! Storage publishes a block before an asset transaction makes it reachable.
pub use fastctx_types::sync::manifest::*;

use crate::{
    cursor::Revision,
    identity::OperationId,
    store::{FutureResult, access::AccessContext},
};

#[derive(Clone, Debug)]
pub struct Pin {
    pub operation: OperationId,
    pub root: BlobId,
    pub generation: Revision,
}

/// A completed block is verified and durably published, but is not an asset
/// until a mutation links it. The durable pin protects that publication gap.
#[derive(Clone, Debug)]
pub struct PublishedBlock {
    pub block: Block,
    pub pin: Pin,
}

/// Implementations stream through bounded buffers. No method accepts an entire
/// file or hands a mutable object-store path to business code.
pub trait BlockStore: Send + Sync {
    fn put(
        &self,
        context: AccessContext,
        operation: OperationId,
        block: Block,
        encoded: Vec<u8>,
    ) -> FutureResult<'_, PublishedBlock>;
    fn read(&self, context: AccessContext, id: BlobId) -> FutureResult<'_, Vec<u8>>;
    /// Release only after a durable root reference or a recorded cancellation.
    fn release(&self, context: AccessContext, pin: Pin) -> FutureResult<'_, ()>;
    /// A bounded pass rechecks references and pins before removing candidates.
    fn collect(&self, max_blocks: u32) -> FutureResult<'_, u32>;
}
