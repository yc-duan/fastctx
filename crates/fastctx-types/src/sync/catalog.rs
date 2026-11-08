//! Object heads, source facts and catalog pages carry separate revisions.

use super::{
    cursor::{Completeness, Revision, ScopeCursor, Sequence, SnapshotFence},
    identity::{DeviceId, ObjectId, ObjectKey, OperationId, ProducerEpoch, ScopeKey, StreamId},
    manifest::BlobId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    Asset,
    Runtime,
    Derived,
    Scratch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Authority {
    Hub,
    Device { device: DeviceId, epoch: ProducerEpoch },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Live,
    Removed,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectHead {
    pub key: ObjectKey,
    pub kind: String,
    pub schema: u16,
    pub acl_revision: Revision,
    pub authority: Authority,
    /// Latest authority-allocated state revision, including retained conflicts.
    pub revision: Revision,
    /// The selected body version; it need not be the largest allocated revision.
    pub current_version: Revision,
    pub content: Option<BlobId>,
    /// All retained versions remain discoverable after catalog feed compaction.
    pub history: Option<BlobId>,
    pub lifecycle: Lifecycle,
    pub completeness: Completeness,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogPage {
    pub fence: SnapshotFence,
    pub objects: Vec<ObjectHead>,
    pub next: Option<ObjectId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogChange {
    pub sequence: Sequence,
    pub operation: OperationId,
    pub head: ObjectHead,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum FeedEntry {
    Changed {
        change: Box<CatalogChange>,
    },
    /// Explicitly covers excluded sequence numbers under exactly this view.
    Skipped {
        after: Sequence,
        through: Sequence,
        view: super::identity::ViewId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesPage {
    pub after: ScopeCursor,
    pub through: ScopeCursor,
    pub entries: Vec<FeedEntry>,
    pub more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub sequence: Sequence,
    pub operation: OperationId,
    pub kind: String,
    pub schema: u16,
    pub payload: BlobId,
    pub causes: Vec<RecordRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordRef {
    pub stream: StreamId,
    pub epoch: ProducerEpoch,
    pub sequence: Sequence,
}

/// Authenticated together with its complete source binding. A relay cannot
/// replace the scope, producer or sequence range of a signed batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBatch {
    pub format: u16,
    pub scope: ScopeKey,
    pub stream: StreamId,
    pub producer: DeviceId,
    pub epoch: ProducerEpoch,
    /// A stream is bound to one registered kind/schema for its whole epoch.
    pub kind: String,
    pub schema: u16,
    pub after: Sequence,
    pub through: Sequence,
    pub records: Vec<SourceRecord>,
}
