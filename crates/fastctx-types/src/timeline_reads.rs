//! Reading a conversation's installed timeline through the local node, without
//! the host's record. Fences, anchors and pages are the foundation's own types.
use crate::{
    ResidentId,
    sync::{
        cursor::{HoldingPeriod, LocalSequence},
        identity::ObjectId,
        manifest::ContentRef,
        timeline::{
            ByteRange, ItemRead, ScopeEnd, TimelineAnchor, TimelineChangesResult, TimelineContentResult,
            TimelineCoverage, TimelineFence, TimelineResult,
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
    /// Waits at most thirty seconds for this conversation's installed head to
    /// move, or for its scope's record to say something the reader has not
    /// been told. `closed` is the holding period under which the reader was
    /// last answered `Closed`: while the records still say that, the wait
    /// holds rather than answering it again at once.
    Wait {
        resident: ResidentId,
        after: LocalSequence,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        closed: Option<HoldingPeriod>,
    },
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

/// Answers that say the reader may go on reading name the holding period they
/// were given under. A reader keeps the latest period it was told and takes an
/// answer under an earlier one as late.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineReply {
    Read {
        result: TimelineResult,
        coverage: TimelineCoverage,
        period: HoldingPeriod,
    },
    Item {
        read: ItemRead,
    },
    Moved {
        local: LocalSequence,
        period: HoldingPeriod,
    },
    Unchanged {
        period: HoldingPeriod,
    },
    Changes {
        result: TimelineChangesResult,
        period: HoldingPeriod,
    },
    Content {
        read: TimelineContentResult,
    },
    Released,
    /// The installed record of the conversation's scope ends this reader's
    /// reading: a recorded fact, under the holding period it was read in. Any
    /// call may be answered so. Nothing else says a conversation is gone.
    Closed {
        period: HoldingPeriod,
        end: ScopeEnd,
    },
    /// This conversation has no timeline installed here; its history is read
    /// from the host's record as before.
    Unavailable,
}
