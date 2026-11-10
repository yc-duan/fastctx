//! Reading a conversation's timeline a page at a time.
//!
//! The timeline is the conversation's history as its machine keeps it in
//! its content database: each event an item, in the conversation's order. A
//! viewer reads the last page, reads on towards the start as it is scrolled
//! to, and asks to be told when the timeline moves; it never receives the
//! whole history.

use crate::ResidentId;
use serde::{Deserialize, Serialize};

/// The most events one page holds, whatever is asked.
pub const PAGE_EVENTS: u32 = 400;
/// The most bytes of events one page holds, one event at least.
pub const PAGE_BYTES: u32 = 1024 * 1024;
/// The most bytes one read of an object gives.
pub const OBJECT_BYTES: u32 = 256 * 1024;

/// Where an event stands in a timeline. A reader hands it back as it was
/// given to read on from there; it says nothing else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelinePlace {
    pub order: String,
    pub item: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum TimelineFrom {
    /// The timeline's last events.
    Latest,
    /// The events just before a place.
    Before { place: TimelinePlace },
    /// The events just after a place.
    After { place: TimelinePlace },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum TimelineAsk {
    /// A page of the conversation's events, earliest first.
    Page { resident: ResidentId, from: TimelineFrom, max_events: u32, max_bytes: u32 },
    /// Answered once the timeline's revision is past `after`, or after a
    /// while with the revision it has: the asker asks again.
    Moved { resident: ResidentId, after: u64 },
    /// A range of an event kept whole as an object, named as a page named it.
    Object { resident: ResidentId, object: String, offset: u64, length: u32 },
}

impl TimelineAsk {
    pub fn resident(&self) -> &ResidentId {
        match self {
            Self::Page { resident, .. } | Self::Moved { resident, .. } | Self::Object { resident, .. } => resident,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineTold {
    Page {
        /// The timeline's revision as the page was read; none while the
        /// conversation's timeline is not made yet.
        revision: Option<u64>,
        events: Vec<TimelineEvent>,
        /// Where to read on from towards the start, when there may be more.
        older: Option<TimelinePlace>,
        /// Where to read on from towards the end, when there may be more.
        newer: Option<TimelinePlace>,
    },
    Moved {
        revision: Option<u64>,
    },
    Object {
        /// The bytes asked for, in base64; none when the conversation's
        /// timeline refers to no such object.
        base64: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub place: TimelinePlace,
    /// The timeline's count of records as this event was written: a reader
    /// that holds the event at a lower count takes this one in its place.
    pub sequence: u64,
    #[serde(flatten)]
    pub shown: TimelineShown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "shown", rename_all = "snake_case")]
pub enum TimelineShown {
    /// The event as JSON. Readers decode each event independently, skipping
    /// those they do not understand.
    Event { event: serde_json::Value },
    /// An event too large for a page: how long it is, the event slimmed to
    /// what is drawn of it when that is small, and the object that holds it
    /// whole.
    Oversized { bytes: u64, slim: Option<serde_json::Value>, object: String },
}
