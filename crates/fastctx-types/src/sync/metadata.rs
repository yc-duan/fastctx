//! Typed immutable payloads that declare their exact references: indexes and
//! item descriptors, distinct from user directory manifests.
use super::{
    catalog::{ObjectHead, RecordRef},
    cursor::{Revision, ScopeCursor, StreamCursor, VersionRef},
    identity::{Digest, ObjectKey},
    manifest::{ContentBlock, ContentRef, ManifestChild, ManifestRoot},
    timeline::{TimelineItem, TimelinePosition},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataCodec {
    HistoryV1,
    CatalogV1,
    TimelineV1,
    /// An encoded ItemDisplayV1; references its owned part bodies and overflow root.
    DisplayV1,
    /// An encoded ItemDisplayPartsV1; references the part bodies it owns.
    DisplayPartsV1,
    /// An encoded ItemFactBodyV1; references every complete typed fact part.
    ItemFactsV1,
    /// An encoded ProjectedBody; references exactly its owned content.
    BodyV1,
}

/// The proof identity binds the original domain/signature; its content block
/// stores the exact serialized proof wrapper, whose signed bytes are preserved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofReference {
    pub id: Digest,
    pub payload: ContentBlock,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryState {
    pub version: VersionRef,
    pub proof: ProofReference,
    pub candidate: Option<ContentRef>,
    pub current_version: VersionRef,
    pub current_content: Option<ContentRef>,
}
/// Newest predecessor first, with one to 64 states. Normal appends add one
/// state; bounded future compaction can fill a page without changing its format.
/// A head signs this predecessor root, never a page containing its own proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryLeaf {
    pub object: ObjectKey,
    pub states: Vec<HistoryState>,
    pub previous: Option<ManifestRoot>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogLeaf {
    pub cursor: ScopeCursor,
    pub objects: Vec<ObjectHead>,
    pub proofs: Vec<ProofReference>,
}
/// A snapshot item remains tied to the original source record, independently
/// of the replica's arrival order or a later reconstruction of its projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedTimelineItem {
    pub item: TimelineItem,
    pub record: RecordRef,
    pub payload: ContentBlock,
    pub batch: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineLeaf {
    pub conversation: ObjectKey,
    pub projection: Revision,
    pub through: StreamCursor,
    pub items: Vec<IndexedTimelineItem>,
    pub proofs: Vec<ProofReference>,
}

/// Immutable snapshot watermarks live here, not in every path-copied leaf.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineRoot {
    pub conversation: ObjectKey,
    pub projection: Revision,
    pub through: StreamCursor,
    pub items: Option<ManifestChild>,
    pub lookup: Option<ManifestChild>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineLookupEntry {
    pub item: super::identity::ObjectId,
    pub position: TimelinePosition,
    pub revision: Revision,
}
/// Only a location is repeated; display descriptors stay in the ordered tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineLookupLeaf {
    pub conversation: ObjectKey,
    pub entries: Vec<TimelineLookupEntry>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "page", content = "value", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelinePayload {
    Root(Box<TimelineRoot>),
    Items(Box<TimelineLeaf>),
    ItemLookup(TimelineLookupLeaf),
}
