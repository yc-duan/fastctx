//! Stream pagination and the canonical timeline projection are separate reads.

use super::{
    catalog::SourceRecord,
    cursor::{Revision, Sequence, StreamCursor},
    identity::{Digest, ObjectId, ObjectKey},
    manifest::{ContentRef, ManifestRoot},
    proof::SignedSourceBatch,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Forward,
    Backward,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", content = "sequence", rename_all = "snake_case")]
pub enum StreamPosition {
    Latest,
    After(Sequence),
    Before(Sequence),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamRead {
    pub stream: StreamCursor,
    pub position: StreamPosition,
    pub through: Option<Sequence>,
    pub max_records: u32,
    pub max_bytes: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenRecord {
    pub record: SourceRecord,
    pub batch_proof: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamPage {
    pub fence: StreamCursor,
    pub direction: Direction,
    pub records: Vec<ProvenRecord>,
    pub proofs: Vec<SignedSourceBatch>,
    pub next: Option<Sequence>,
}

/// Allocated by the conversation's canonical projector, not the receiving
/// device. The display path includes this item's creation key, so parents are
/// prefixes of their children and a subtree stays before its next sibling.
/// It is not the host's full causal chain. Creation keys do not change on edit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TimelinePosition {
    pub turn: Sequence,
    pub display_ancestry: Vec<Sequence>,
    pub accepted: Sequence,
    pub item: ObjectId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineItem {
    pub id: ObjectId,
    pub revision: Revision,
    pub turn: ObjectId,
    pub parent: Option<ObjectId>,
    pub position: TimelinePosition,
    pub body: ContentRef,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineFence {
    pub conversation: ObjectKey,
    pub projection: Revision,
    pub through: StreamCursor,
    pub root: ManifestRoot,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", content = "position", rename_all = "snake_case")]
pub enum TimelineAnchor {
    Latest,
    Before(TimelinePosition),
    After(TimelinePosition),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineRead {
    pub conversation: ObjectKey,
    pub fence: Option<TimelineFence>,
    pub anchor: TimelineAnchor,
    pub max_items: u32,
    pub max_bytes: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelinePage {
    pub fence: TimelineFence,
    /// One latest revision per item at the fence, returned in display order.
    pub items: Vec<TimelineItem>,
    pub older: Option<TimelinePosition>,
    pub newer: Option<TimelinePosition>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum TimelineChange {
    Put { item: TimelineItem },
    Retract { item: ObjectId, revision: Revision },
    SelectBranch { root: ManifestRoot, projection: Revision },
}
