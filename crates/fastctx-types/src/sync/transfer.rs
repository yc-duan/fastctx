//! Extensible advertisements precede strict, capability-gated synchronization.

use super::{
    catalog::{CatalogPage, ChangesPage},
    cursor::{BlockRange, ByteCount, DurableAck, HoldingPeriod, PageCursor, Revision, ScopeCursor, SnapshotFence},
    exposure::{ExposureIntent, ShareActivation, SignedExposureReceipt},
    identity::{Digest, FeedEpoch, HubId, ScopeKey, ViewId},
    manifest::{BlobId, Descriptor, Encoding, ManifestReply, ManifestRoot},
    proof::{ConfirmedEpoch, SignedSourceBatch},
    timeline::{StreamPage, StreamRead},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Unknown names are ignored during intersection, not rejected by View serde.
/// Missing or unsupported values disable this capability alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub protocols: Vec<String>,
    pub kinds: Vec<KindVersion>,
    pub envelopes: Vec<String>,
    pub encodings: Vec<String>,
    pub receive_window: ByteCount,
    pub max_page_bytes: u32,
    pub exposure_cleanup: Vec<String>,
    pub hub_signing: Option<SigningAdvertisement>,
    /// Open names; absence disables the separate asset data channel.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub transports: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningAdvertisement {
    pub hub: HubId,
    pub algorithm: String,
    pub public_key: String,
    pub revision: String,
    /// Rotation proofs are parsed only by the selected signing scheme.
    pub rotations: Vec<serde_json::Value>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct KindVersion {
    pub kind: String,
    pub schema: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectedProtocol {
    #[serde(rename = "fastctx-sync/v1")]
    V1,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectedEnvelope {
    #[serde(rename = "plain/v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub protocol: SelectedProtocol,
    pub envelope: SelectedEnvelope,
    pub encoding: Encoding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<SelectedTransport>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectedTransport {
    #[serde(rename = "asset-transfer/v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewDefinition {
    pub scope: ScopeKey,
    pub acl_revision: Revision,
    pub projection_schema: u16,
    pub kinds: Vec<KindVersion>,
}
impl ViewDefinition {
    pub fn id(&self) -> Result<ViewId, String> {
        let mut kinds = self.kinds.clone();
        kinds.sort();
        if kinds.windows(2).any(|pair| pair[0].kind == pair[1].kind) {
            return Err("a view selects one schema for each kind".into());
        }
        let mut hash = Sha256::new();
        hash.update(b"fastctx.sync.view.v1\0");
        hash.update(self.scope.hub.bytes());
        hash.update(self.scope.scope.as_str().as_bytes());
        hash.update(self.acl_revision.0.to_be_bytes());
        hash.update(self.projection_schema.to_be_bytes());
        for kind in kinds {
            hash.update((kind.kind.len() as u64).to_be_bytes());
            hash.update(kind.kind.as_bytes());
            hash.update(kind.schema.to_be_bytes());
        }
        Ok(ViewId::from_bytes(hash.finalize().into()))
    }
}

/// Added kinds receive fenced snapshots; existing kinds resume after the old N.
/// Removal invalidates the affected data before a new view becomes readable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewTransition {
    pub previous: ScopeCursor,
    pub next: ViewDefinition,
    pub added: Vec<KindVersion>,
    pub removed: Vec<KindVersion>,
    pub snapshots: Vec<SnapshotFence>,
}

/// Routing purpose is distinct from Hands/Agents authorization.
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
    /// Sent only after the legacy-readable handshake/View advertised support.
    Hello {
        capabilities: Capabilities,
    },
    Selected {
        selection: Selection,
    },
    /// Control-plane messages, sent only after selecting asset-transfer/v1.
    OpenSyncSession {
        request: super::session::SessionOpen,
    },
    SessionReady {
        ready: super::session::SessionReady,
    },
    SessionClose {
        close: super::session::SessionClose,
    },
    GetScopes {
        request: super::scope::ScopeDiscoveryRead,
    },
    ScopesPage {
        page: super::scope::ScopesPage,
    },
    Cursors {
        entries: Vec<ScopeCursor>,
        more: bool,
    },
    ViewChanged {
        transition: ViewTransition,
    },
    Catalog {
        holding: HoldingPeriod,
        cursor: Option<PageCursor>,
        scope: ScopeKey,
        kinds: Vec<KindVersion>,
    },
    CatalogPage {
        holding: HoldingPeriod,
        page: CatalogPage,
    },
    Changes {
        holding: HoldingPeriod,
        after: ScopeCursor,
        /// Selected kind/schema set whose digest must equal `after.view`.
        /// Omission selects the full view advertised by scope discovery (v1
        /// projection schema). A cursor without a view still needs reconciliation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        view: Option<ViewDefinition>,
        max_bytes: u32,
    },
    ChangesPage {
        holding: HoldingPeriod,
        page: ChangesPage,
    },
    Source {
        holding: HoldingPeriod,
        batch: SignedSourceBatch,
    },
    SourceResult {
        result: super::source_acceptance::SourceAccepted,
    },
    ProbeSource {
        upload: super::source_transfer::SourceTransfer,
        content: super::manifest::ContentRef,
    },
    SourcePresence {
        upload: super::source_transfer::SourceTransfer,
        content: super::manifest::ContentRef,
        present: bool,
    },
    /// The exact page bytes were offered as `page` under this batch operation.
    /// This request explicitly interprets those bytes as a manifest, without
    /// putting a second escaped copy of the page inside a metadata envelope.
    PublishSourceManifest {
        upload: super::source_transfer::SourceTransfer,
        page: super::manifest::ContentBlock,
    },
    Submit {
        request: super::mutation::Submission,
    },
    GetOperation {
        hub: HubId,
        operation: super::identity::OperationId,
    },
    Operation {
        hub: HubId,
        receipt: super::mutation::OperationReceipt,
    },
    RestoreAccepted {
        proof: super::proof::SignedAcceptanceFragment,
    },
    StreamRange {
        holding: HoldingPeriod,
        request: StreamRead,
    },
    StreamPage {
        holding: HoldingPeriod,
        page: StreamPage,
    },
    GetEpoch {
        scope: ScopeKey,
        lookup: super::proof::EpochLookup,
    },
    Epoch {
        descriptor: ConfirmedEpoch,
    },
    GetManifest {
        holding: HoldingPeriod,
        root: ManifestRoot,
    },
    Manifest {
        holding: HoldingPeriod,
        reply: ManifestReply,
    },
    GetDescriptor {
        holding: HoldingPeriod,
        blob: BlobId,
    },
    Descriptor {
        holding: HoldingPeriod,
        descriptor: Descriptor,
    },
    WantBlocks {
        holding: HoldingPeriod,
        manifest: ManifestRoot,
        missing: Vec<BlockRange>,
    },
    Blocks {
        holding: HoldingPeriod,
        manifest: ManifestRoot,
        blocks: Vec<super::manifest::IndexedBlock>,
        remaining: Vec<BlockRange>,
    },
    Acknowledge {
        durable: DurableAck,
    },
    Advanced {
        cursor: ScopeCursor,
    },
    Reset {
        scope: ScopeKey,
        epoch: FeedEpoch,
        reason: String,
    },
    Revoked {
        holding: HoldingPeriod,
    },
    PrepareExposure {
        intent: ExposureIntent,
    },
    ExposureCleaned {
        receipt: SignedExposureReceipt,
    },
    ShareState {
        state: ShareActivation,
    },
}
impl Message {
    pub fn purpose(&self) -> Purpose {
        match self {
            Self::PrepareExposure { .. } | Self::ExposureCleaned { .. } | Self::ShareState { .. } => {
                Purpose::ExecutionControl
            }
            Self::Hello { .. }
            | Self::Selected { .. }
            | Self::OpenSyncSession { .. }
            | Self::SessionReady { .. }
            | Self::SessionClose { .. }
            | Self::GetScopes { .. }
            | Self::ScopesPage { .. }
            | Self::Cursors { .. }
            | Self::ViewChanged { .. }
            | Self::Catalog { .. }
            | Self::CatalogPage { .. }
            | Self::Changes { .. }
            | Self::ChangesPage { .. }
            | Self::Source { .. }
            | Self::SourceResult { .. }
            | Self::ProbeSource { .. }
            | Self::SourcePresence { .. }
            | Self::PublishSourceManifest { .. }
            | Self::Submit { .. }
            | Self::GetOperation { .. }
            | Self::Operation { .. }
            | Self::RestoreAccepted { .. }
            | Self::StreamRange { .. }
            | Self::StreamPage { .. }
            | Self::GetEpoch { .. }
            | Self::Epoch { .. }
            | Self::GetManifest { .. }
            | Self::Manifest { .. }
            | Self::GetDescriptor { .. }
            | Self::Descriptor { .. }
            | Self::WantBlocks { .. }
            | Self::Blocks { .. }
            | Self::Acknowledge { .. }
            | Self::Advanced { .. }
            | Self::Reset { .. }
            | Self::Revoked { .. } => Purpose::AssetSync,
        }
    }
}

/// The selected representation and holding period fence every binary frame.
/// Verify this frame's bytes, the whole representation, and decoded content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockFrame {
    pub holding: HoldingPeriod,
    pub blob: BlobId,
    pub representation: Digest,
    pub offset: ByteCount,
    pub payload_bytes: u32,
    pub frame_digest: Digest,
}
