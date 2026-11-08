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
    pub revision: Revision,
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
pub struct ChangesPage {
    pub after: ScopeCursor,
    pub through: ScopeCursor,
    pub changes: Vec<CatalogChange>,
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
    pub scope: ScopeKey,
    pub stream: StreamId,
    pub producer: DeviceId,
    pub epoch: ProducerEpoch,
    pub after: Sequence,
    pub records: Vec<SourceRecord>,
}
