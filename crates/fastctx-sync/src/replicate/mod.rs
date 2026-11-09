//! A device's side of asset sync with one hub. Drivers speak only through
//! [`Upstream`]; the network runner and the in-process loopback implement it,
//! and neither turns a peer's message into a permission.
mod directory;

pub use directory::{DirectoryFollower, Discovered};

use crate::{
    cursor::{DurableAck, HoldingPeriod},
    identity::{ObjectKey, OperationId},
    manifest::{ContentBlock, Representation},
    store::FutureResult,
    transfer::{KindVersion, Message},
};

/// One block of an object, under the device's current holding of its scope.
#[derive(Clone, Debug)]
pub struct BlockRequest {
    pub holding: HoldingPeriod,
    pub object: ObjectKey,
    pub kind: KindVersion,
    pub block: ContentBlock,
}

/// One block a device stages at the hub under `operation`, before it submits
/// the batch or mutation that retains it.
#[derive(Clone, Debug)]
pub struct BlockOffer {
    pub holding: HoldingPeriod,
    pub object: ObjectKey,
    pub kind: KindVersion,
    pub operation: OperationId,
    pub representation: Representation,
    pub encoded: Vec<u8>,
}

pub trait Upstream: Send + Sync {
    /// One bounded metadata request and the hub's answer to it.
    fn exchange(&self, request: Message) -> FutureResult<'_, Message>;
    /// The decoded bytes of one block.
    fn fetch(&self, request: BlockRequest) -> FutureResult<'_, Vec<u8>>;
    /// Stages one block at the hub; the acknowledgement covers that block only.
    fn offer(&self, offer: BlockOffer) -> FutureResult<'_, DurableAck>;
}
