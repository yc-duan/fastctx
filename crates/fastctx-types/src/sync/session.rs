//! Capability-gated asset sessions on hub and direct-device data connections.
//!
//! These are untrusted wire values, never authorization tokens. The service
//! binds a session to an authenticated control connection incarnation and its
//! current directory. Each request, offered part and publication must check
//! object membership, kind/schema, read/holding purpose and current ACL again.
//! Direct devices serve content and verified timeline projections, never
//! authority acceptance or change feeds; their ACK holder cannot be the hub.

pub use super::identity::{MessageId, PartId, SessionId, TransferId};
use super::{
    cursor::{BlockRange, ByteCount, Count, DurableAck, HoldingPeriod, Sequence},
    identity::{Digest, ObjectKey, OperationId, ProducerEpoch, ScopeKey},
    manifest::{BlobId, ContentBlock, Encoding, ManifestRoot, Representation},
    transfer::{BlockFrame, KindVersion, Purpose},
};
use serde::{Deserialize, Serialize};

pub const MAX_FRAME_BYTES: u32 = 64 * 1024;
pub const MAX_CONTROL_BYTES: u32 = 16 * 1024;
pub const MAX_DATA_HEADER_BYTES: u32 = 4 * 1024;
pub const MAX_RANGES: usize = 256;
pub const TRANSPORT_PROTOCOL: &str = "asset-transfer/v1";

/// Negotiated per connection, within the receiver's already reserved budget.
/// Counts bound concurrent work, not the number of assets that may be synced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportLimits {
    pub max_frame_bytes: u32,
    pub max_control_bytes: u32,
    pub max_metadata_bytes: u32,
    pub max_active_transfers: u32,
    pub max_active_parts: u32,
}
impl TransportLimits {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_frame_bytes <= 16
            || self.max_frame_bytes > MAX_FRAME_BYTES
            || self.max_control_bytes == 0
            || self.max_control_bytes > MAX_CONTROL_BYTES
            || self.max_control_bytes > self.max_frame_bytes - 16
            || self.max_metadata_bytes == 0
            || self.max_metadata_bytes > super::PAGE_BYTES
            || self.max_active_transfers == 0
            || self.max_active_parts == 0
        {
            return Err("invalid asset transport limits".into());
        }
        Ok(())
    }
    /// A peer cannot enlarge any of the local receiver's limits.
    pub fn intersect(&self, other: &Self) -> Result<Self, String> {
        self.validate()?;
        other.validate()?;
        Ok(Self {
            max_frame_bytes: self.max_frame_bytes.min(other.max_frame_bytes),
            max_control_bytes: self.max_control_bytes.min(other.max_control_bytes),
            max_metadata_bytes: self.max_metadata_bytes.min(other.max_metadata_bytes),
            max_active_transfers: self.max_active_transfers.min(other.max_active_transfers),
            max_active_parts: self.max_active_parts.min(other.max_active_parts),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOpen {
    pub session: SessionId,
    pub binding: SessionBindingRequest,
    pub limits: TransportLimits,
}
/// Scope binding is only a boundary, not a grant to every kind in the scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "binding", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionBindingRequest {
    Object { object: ObjectKey, kind: KindVersion },
    Scope { scope: ScopeKey },
}
impl SessionBindingRequest {
    pub fn binding_purpose(&self) -> Purpose {
        match self {
            Self::Object { .. } | Self::Scope { .. } => Purpose::AssetSync,
        }
    }
    pub fn scope(&self) -> &ScopeKey {
        match self {
            Self::Object { object, .. } => &object.scope,
            Self::Scope { scope } => scope,
        }
    }
    pub fn contains(&self, request: &TransferRequest) -> bool {
        match self {
            Self::Object { object, kind } => object == &request.object && kind == &request.kind,
            Self::Scope { scope } => scope == &request.object.scope,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionReady {
    pub session: SessionId,
    pub holding: HoldingPeriod,
    pub limits: TransportLimits,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionClose {
    pub session: SessionId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Upload,
    Download,
}
/// Byte scheduling weights are 4:2:1, separate from durable task slot weights.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPriority {
    Interactive,
    Recent,
    Background,
}
impl TransferPriority {
    pub fn weight(self) -> u32 {
        match self {
            Self::Interactive => 4,
            Self::Recent => 2,
            Self::Background => 1,
        }
    }
}
/// Why a device stages content before submitting its owning operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case", deny_unknown_fields)]
pub enum UploadCause {
    Source { epoch: ProducerEpoch },
    Mutation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferRequest {
    pub session: SessionId,
    pub transfer: TransferId,
    pub operation: OperationId,
    pub direction: TransferDirection,
    pub object: ObjectKey,
    pub kind: KindVersion,
    pub holding: HoldingPeriod,
    pub target: TransferTarget,
    pub representation: RepresentationChoice,
    pub priority: TransferPriority,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransferTarget {
    Single {
        content: ContentBlock,
    },
    /// Ordered Blocks/Branch trees only, never an ad hoc directory DFS order.
    ManifestRanges {
        root: ManifestRoot,
        ranges: Vec<BlockRange>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepresentationChoice {
    Encodings {
        preferred: Vec<Encoding>,
    },
    /// Only valid for Single; the digest identifies the encoded representation.
    Exact {
        content: BlobId,
        representation: Digest,
    },
}
impl TransferRequest {
    /// Checks shape and scope consistency, not permissions or root membership.
    pub fn validate(&self) -> Result<(), String> {
        validate_kind(&self.kind)?;
        if self.holding.scope != self.object.scope {
            return Err("holding scope differs from object".into());
        }
        match &self.target {
            TransferTarget::Single { content } => {
                if content.id.domain != self.object.scope || content.decoded_bytes > super::BLOCK_BYTES {
                    return Err("invalid single-block target".into());
                }
            }
            TransferTarget::ManifestRanges { root, ranges } => {
                if root.0.domain != self.object.scope || ranges.is_empty() || ranges.len() > MAX_RANGES {
                    return Err("invalid manifest ranges".into());
                }
                for range in ranges {
                    range.validate()?;
                }
                if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
                    return Err("block ranges must be sorted and disjoint".into());
                }
            }
        }
        match &self.representation {
            RepresentationChoice::Encodings { preferred } => {
                if preferred.is_empty() || preferred.len() > 2 || (preferred.len() == 2 && preferred[0] == preferred[1])
                {
                    return Err("encoding preferences must be nonempty and unique".into());
                }
            }
            RepresentationChoice::Exact { content, .. } => {
                if !matches!(&self.target, TransferTarget::Single { content: block } if &block.id == content) {
                    return Err("exact representation requires the same single block".into());
                }
            }
        }
        Ok(())
    }
    /// Persistent idempotency binding excludes the connection-local session and
    /// mutable scheduling preference. Encoding changes use new parts, not a new
    /// target; callers persist and compare this digest before reusing a transfer.
    pub fn fingerprint(&self) -> Result<Digest, String> {
        self.validate()?;
        let mut bytes = b"fastctx.sync.transfer-request.v1\0".to_vec();
        serde_json::to_writer(
            &mut bytes,
            &(&self.transfer, &self.operation, self.direction, &self.object, &self.kind, &self.holding, &self.target),
        )
        .map_err(|error| error.to_string())?;
        Ok(Digest::of(&bytes))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartOffer {
    pub session: SessionId,
    pub transfer: TransferId,
    pub part: PartId,
    pub block_index: Option<Count>,
    pub representation: Representation,
}
/// The receiver alone reports its persisted prefix of the offered representation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartReady {
    pub session: SessionId,
    pub transfer: TransferId,
    pub part: PartId,
    pub durable_offset: ByteCount,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataHeader {
    pub session: SessionId,
    pub transfer: TransferId,
    pub part: PartId,
    /// Strictly increasing within this part in this session.
    pub frame: Sequence,
    pub block: BlockFrame,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "stream", rename_all = "snake_case", deny_unknown_fields)]
pub enum CreditStream {
    Metadata,
    Transfer { transfer: TransferId },
}
/// Cumulative payload allowance, including retransmissions. Never an ACK.
/// A receiver reserves memory before granting it and returns credit on consume,
/// independently of staging fsync, Stored checkpoints and final publication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credit {
    pub session: SessionId,
    pub stream: CreditStream,
    pub allowed_payload_total: ByteCount,
}
/// A continuous prefix whose bytes and checkpoint are durably stored. It does
/// not certify the whole hash, expose the asset, or release its source root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stored {
    pub session: SessionId,
    pub transfer: TransferId,
    pub part: PartId,
    pub durable_offset: ByteCount,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartEnd {
    pub session: SessionId,
    pub transfer: TransferId,
    pub part: PartId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartCommitted {
    pub session: SessionId,
    pub transfer: TransferId,
    pub part: PartId,
    /// Must be Block; the receiver, not the sender, is the holder.
    pub ack: DurableAck,
    /// Actual stored representation; an existing verified slot may be reused.
    pub stored_representation: Digest,
}
/// All requested parts finished; this is not an object-completeness ACK.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferFinished {
    pub session: SessionId,
    pub transfer: TransferId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferCancel {
    pub session: SessionId,
    pub transfer: TransferId,
    pub operation: OperationId,
}
/// Sent after persistent cancellation and in-flight IO cleanup. Late data cannot
/// recreate the transfer. Cancelling demand never deletes an accepted asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferCancelled {
    pub session: SessionId,
    pub transfer: TransferId,
    pub operation: OperationId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferError {
    pub session: SessionId,
    pub transfer: Option<TransferId>,
    pub part: Option<PartId>,
    pub code: TransferErrorCode,
    pub retry_after_ms: Option<u32>,
    /// At most 1024 UTF-8 bytes, with no keys, private paths or existence oracle.
    pub detail: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferErrorCode {
    UnsupportedProtocol,
    UnsupportedEncoding,
    InvalidRequest,
    TooLarge,
    NotAccessible,
    StaleHolding,
    Revoked,
    StaleSession,
    TransferConflict,
    OffsetMismatch,
    RepresentationChanged,
    CorruptFrame,
    CorruptRepresentation,
    CorruptContent,
    Busy,
    DiskFull,
    SourceUnavailable,
    Cancelled,
    OutcomeUnknown,
}

/// Header for exact serialized Message bytes; signatures are never reserialized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataPart {
    pub session: SessionId,
    pub message_id: MessageId,
    pub offset: ByteCount,
    pub total_bytes: ByteCount,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum TransportControl {
    SessionOpen(SessionOpen),
    SessionReady(SessionReady),
    SessionClose(SessionClose),
    TransferRequest(TransferRequest),
    PartOffer(PartOffer),
    PartReady(PartReady),
    Credit(Credit),
    Stored(Stored),
    PartEnd(PartEnd),
    PartCommitted(PartCommitted),
    TransferFinished(TransferFinished),
    TransferCancel(TransferCancel),
    TransferCancelled(TransferCancelled),
    Error(TransferError),
}
impl TransportControl {
    pub fn session(&self) -> &SessionId {
        match self {
            Self::SessionOpen(v) => &v.session,
            Self::SessionReady(v) => &v.session,
            Self::SessionClose(v) => &v.session,
            Self::TransferRequest(v) => &v.session,
            Self::PartOffer(v) => &v.session,
            Self::PartReady(v) => &v.session,
            Self::Credit(v) => &v.session,
            Self::Stored(v) => &v.session,
            Self::PartEnd(v) => &v.session,
            Self::PartCommitted(v) => &v.session,
            Self::TransferFinished(v) => &v.session,
            Self::TransferCancel(v) => &v.session,
            Self::TransferCancelled(v) => &v.session,
            Self::Error(v) => &v.session,
        }
    }
    pub fn purpose(&self) -> Purpose {
        match self {
            Self::SessionOpen(..)
            | Self::SessionReady(..)
            | Self::SessionClose(..)
            | Self::TransferRequest(..)
            | Self::PartOffer(..)
            | Self::PartReady(..)
            | Self::Credit(..)
            | Self::Stored(..)
            | Self::PartEnd(..)
            | Self::PartCommitted(..)
            | Self::TransferFinished(..)
            | Self::TransferCancel(..)
            | Self::TransferCancelled(..)
            | Self::Error(..) => Purpose::AssetSync,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::SessionOpen(v) => {
                v.limits.validate()?;
                if let SessionBindingRequest::Object { kind, .. } = &v.binding {
                    validate_kind(kind)?;
                }
            }
            Self::SessionReady(v) => v.limits.validate()?,
            Self::TransferRequest(v) => v.validate()?,
            Self::PartOffer(v) => v.representation.validate()?,
            Self::PartCommitted(v) => {
                let DurableAck::Block { holding, object, kind, schema, content } = &v.ack else {
                    return Err("a part commit requires a block ACK".into());
                };
                validate_kind(&KindVersion { kind: kind.clone(), schema: *schema })?;
                if holding.scope != object.scope
                    || content.id.domain != object.scope
                    || content.decoded_bytes > super::BLOCK_BYTES
                {
                    return Err("invalid block ACK scope or size".into());
                }
            }
            Self::Error(v) if v.detail.len() > 1024 || (v.part.is_some() && v.transfer.is_none()) => {
                return Err("invalid transport error bounds".into());
            }
            _ => {}
        }
        Ok(())
    }
}
fn validate_kind(kind: &KindVersion) -> Result<(), String> {
    if kind.kind.is_empty() || kind.kind.len() > 256 || kind.schema == 0 {
        Err("invalid asset kind/schema".into())
    } else {
        Ok(())
    }
}
