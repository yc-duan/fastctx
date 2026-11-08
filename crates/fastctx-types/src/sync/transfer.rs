//! Capability-gated control messages. Large bodies use a separate binary stream.

use super::{
    catalog::{CatalogPage, ChangesPage, SourceBatch},
    cursor::{BlockRange, ByteCount, DurableAck, PageCursor, Revision, ScopeCursor, StreamCursor},
    identity::{FeedEpoch, ScopeKey},
    manifest::{BlobId, Encoding},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub protocol: u16,
    /// Installed handlers, not every kind whose type this binary can decode.
    pub kinds: Vec<KindVersion>,
    pub envelope: u16,
    pub encodings: Vec<Encoding>,
    pub receive_window: ByteCount,
    pub max_page_bytes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindVersion {
    pub kind: String,
    pub schema: u16,
}

/// This is protocol routing, not the existing Hands/Agents authorization purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    AssetSync,
    ExecutionControl,
    Observation,
    Bootstrap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum Message {
    Hello { capabilities: Capabilities },
    Cursors { entries: Vec<ScopeCursor>, more: bool },
    Catalog { cursor: Option<PageCursor>, scope: ScopeKey },
    CatalogPage { page: CatalogPage },
    Changes { after: ScopeCursor, max_bytes: u32 },
    ChangesPage { page: ChangesPage },
    Source { batch: SourceBatch },
    StreamRange { after: StreamCursor, max_bytes: u32 },
    WantBlocks { manifest: BlobId, missing: Vec<BlockRange> },
    Acknowledge { durable: DurableAck },
    Advanced { cursor: ScopeCursor },
    Reset { scope: ScopeKey, epoch: FeedEpoch, reason: String },
    Revoked { scope: ScopeKey, acl_revision: Revision },
}

impl Message {
    /// Every new message must choose a routing purpose here.
    pub fn purpose(&self) -> Purpose {
        match self {
            Self::Hello { .. }
            | Self::Cursors { .. }
            | Self::Catalog { .. }
            | Self::CatalogPage { .. }
            | Self::Changes { .. }
            | Self::ChangesPage { .. }
            | Self::Source { .. }
            | Self::StreamRange { .. }
            | Self::WantBlocks { .. }
            | Self::Acknowledge { .. }
            | Self::Advanced { .. }
            | Self::Reset { .. }
            | Self::Revoked { .. } => Purpose::AssetSync,
        }
    }
}

/// One frame of an already authorized block transfer. The payload is binary,
/// follows this header on the data connection, and is not base64 control JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockFrame {
    pub blob: BlobId,
    pub offset: ByteCount,
    pub payload_bytes: u32,
    pub acl_revision: Revision,
}
