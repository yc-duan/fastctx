//! A device's side of asset sync with one hub. Drivers speak only through
//! [`Upstream`]; the network runner and the in-process loopback implement it,
//! and neither turns a peer's message into a permission.
mod directory;
mod follow;
mod source_upload;
pub use source_upload::{
    SourceUploadKey, SourceUploadPage, SourceUploadPhase, SourceUploadProgress, SourceUploadState, SourceUploadStop,
    SourceUploader,
};
mod received;
pub use received::{Received, decode_json, decode_message};

pub use directory::{DirectoryFollower, Discovered, DiscoveredPage};
pub use follow::{FollowDiscovery, FollowPage, FollowProgress, ReplicaFollower};

use crate::{
    cursor::{DurableAck, HoldingPeriod},
    identity::{ObjectKey, OperationId},
    manifest::{ContentBlock, Representation},
    store::FutureResult,
    transfer::{KindVersion, Message},
};

/// One block of an object, under the device's current holding of its scope.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BlockRequest {
    pub holding: HoldingPeriod,
    pub object: ObjectKey,
    pub kind: KindVersion,
    pub block: ContentBlock,
    pub root: crate::manifest::ContentRef,
}

/// One block a device stages at the hub under `operation`, before it submits
/// the batch or mutation that retains it.
#[derive(Clone, Debug)]
pub struct BlockOffer {
    pub holding: HoldingPeriod,
    pub object: ObjectKey,
    pub kind: KindVersion,
    pub operation: OperationId,
    pub cause: fastctx_types::sync::session::UploadCause,
    pub representation: Representation,
    pub encoded: Vec<u8>,
}

pub trait Upstream: Send + Sync {
    /// One bounded metadata request and the hub's answer to it. The returned
    /// guard retains the decoded answer's byte admission until consumption ends.
    fn exchange(&self, request: Message) -> FutureResult<'_, Received<Message>>;
    /// The verified, decoded bytes of one block, retaining their byte admission.
    fn fetch(&self, request: BlockRequest) -> FutureResult<'_, Received<Vec<u8>>>;
    /// Stages one block at the hub; the acknowledgement covers that block only.
    fn offer(&self, offer: BlockOffer) -> FutureResult<'_, DurableAck>;
}
