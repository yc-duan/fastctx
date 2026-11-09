//! Reading a conversation's installed timeline through the local node, without
//! the host's record. Fences, anchors and pages are the foundation's own types.
use crate::{
    ResidentId,
    sync::{
        identity::ObjectId,
        timeline::{ItemRead, TimelineAnchor, TimelineFence, TimelineResult},
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum TimelineCall {
    /// A page of `resident`'s timeline; without a fence, from its latest snapshot.
    Read { resident: ResidentId, fence: Option<TimelineFence>, anchor: TimelineAnchor, max_items: u32, max_bytes: u32 },
    /// One item of the snapshot `fence` names.
    Item { resident: ResidentId, fence: TimelineFence, item: ObjectId },
    /// Ends `fence`'s hold once its reader is done with it.
    Release { resident: ResidentId, fence: TimelineFence },
}
impl TimelineCall {
    pub fn resident(&self) -> &ResidentId {
        match self {
            Self::Read { resident, .. } | Self::Item { resident, .. } | Self::Release { resident, .. } => resident,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum TimelineReply {
    Read {
        result: TimelineResult,
    },
    Item {
        read: ItemRead,
    },
    Released,
    /// This conversation has no timeline installed here; its history is read
    /// from the host's record as before.
    Unavailable,
}
