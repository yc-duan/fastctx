//! Reading a conversation's installed timeline through the local node, without
//! the host's record. Fences, anchors and pages are the foundation's own types.
use crate::{
    ResidentId,
    sync::{
        cursor::LocalSequence,
        identity::ObjectId,
        manifest::ContentRef,
        timeline::{
            ByteRange, ItemRead, TimelineAnchor, TimelineChangesResult, TimelineContentResult, TimelineCoverage,
            TimelineFence, TimelineResult,
        },
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum TimelineCall {
    /// A page of `resident`'s timeline; without a fence, from its latest snapshot.
    Read { resident: ResidentId, fence: Option<TimelineFence>, anchor: TimelineAnchor, max_items: u32, max_bytes: u32 },
    /// One item of the snapshot `fence` names.
    Item { resident: ResidentId, fence: TimelineFence, item: ObjectId },
    /// Waits at most thirty seconds for this conversation's installed head to move.
    Wait { resident: ResidentId, after: LocalSequence },
    /// ID-ordered patches between two held snapshots; None acquires the latest `to`.
    Changes {
        resident: ResidentId,
        from: Box<TimelineFence>,
        to: Option<Box<TimelineFence>>,
        after: Option<ObjectId>,
        max_items: u32,
        max_bytes: u32,
    },
    /// A bounded range of logical content owned by an item at this snapshot.
    Content {
        resident: ResidentId,
        fence: Box<TimelineFence>,
        item: ObjectId,
        content: ContentRef,
        range: Option<ByteRange>,
        /// Ancestors from an item-owned root through the target's parent.
        /// An empty path requires direct ownership; None uses bounded legacy lookup.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<Vec<ContentRef>>,
    },
    /// Ends `fence`'s hold once its reader is done with it.
    Release { resident: ResidentId, fence: TimelineFence },
}
impl TimelineCall {
    pub fn resident(&self) -> &ResidentId {
        match self {
            Self::Read { resident, .. }
            | Self::Item { resident, .. }
            | Self::Release { resident, .. }
            | Self::Wait { resident, .. }
            | Self::Changes { resident, .. }
            | Self::Content { resident, .. } => resident,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineReply {
    Read {
        result: TimelineResult,
        coverage: TimelineCoverage,
    },
    Item {
        read: ItemRead,
    },
    Moved {
        local: LocalSequence,
    },
    Unchanged,
    Changes {
        result: TimelineChangesResult,
    },
    Content {
        read: TimelineContentResult,
    },
    Released,
    /// This conversation has no timeline installed here; its history is read
    /// from the host's record as before.
    Unavailable,
}
