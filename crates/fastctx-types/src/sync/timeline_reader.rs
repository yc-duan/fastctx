//! Snapshot patches and item-owned logical content for bounded timeline readers.
use super::{
    cursor::{ByteCount, LocalSequence},
    identity::ObjectId,
    manifest::ContentRef,
    progress::StreamProgress,
    timeline::{TimelineChange, TimelineFence},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TimelineCoverage {
    Complete,
    Incomplete { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineWait {
    Moved { local: LocalSequence },
    Unchanged,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineChangesRead {
    pub from: TimelineFence,
    pub to: Option<TimelineFence>,
    pub after: Option<ObjectId>,
    pub max_items: u32,
    pub max_bytes: u32,
}

/// These are snapshot patches, not canonical events. A retract identifies the
/// revision removed from `from`; it does not invent a new source revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineChangesResult {
    Page { to: Box<TimelineFence>, changes: Vec<TimelineChange>, next: Option<ObjectId> },
    Pending { progress: Option<StreamProgress> },
    ReanchorNeeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Offsets are relative to the requested logical content, including when that
/// content names a subtree whose persisted coordinates start after zero.
pub struct ByteRange {
    pub offset: ByteCount,
    pub bytes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineContentRead {
    pub fence: TimelineFence,
    pub item: ObjectId,
    pub content: ContentRef,
    pub range: Option<ByteRange>,
}

/// Bytes are decoded logical content: a metadata page's payload, or a range of
/// the byte tree it names. Manifest envelopes never become application content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineContentResult {
    Content { base64: String, offset: ByteCount, total: ByteCount, next: Option<ByteCount> },
    Pending { progress: Option<StreamProgress> },
    ReanchorNeeded,
}
