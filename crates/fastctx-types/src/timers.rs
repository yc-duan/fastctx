//! Durable, named deadlines and their idempotent actions.

use crate::{NoticeId, ResidentId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Owner {
    Conversation { resident: ResidentId },
    Goal { resident: ResidentId },
    Sentinel { id: String },
}

impl Owner {
    pub fn conversation(&self) -> Option<&ResidentId> {
        match self {
            Self::Conversation { resident } | Self::Goal { resident } => Some(resident),
            Self::Sentinel { .. } => None,
        }
    }
}

/// Each action must recognize `Timer::delivery` on repeated attempts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Tell { resident: ResidentId, text: String },
    /// A sentinel's wake-up, handed to the sentinel runtime until it acknowledges it.
    Sentinel { action: crate::sentinel::TimerAction },
    Goal { resident: ResidentId, goal: String, work: NoticeId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Timestamp>,
    pub owner: Owner,
    pub due: Timestamp,
    pub action: Action,
    /// Identifies this setting of the timer; replacing its id starts a new delivery.
    pub delivery: NoticeId,
    pub attempts: u32,
    pub next_attempt: Timestamp,
    /// Frozen before the first effect, so retries have identical words and lateness.
    pub fired_at: Option<Timestamp>,
    pub error: Option<String>,
}

impl Timer {
    pub fn late_ms(&self) -> i64 {
        self.fired_at.map_or(0, |at| (at.as_millisecond() - self.due.as_millisecond()).max(0))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum TimerCall {
    Set { id: String, owner: Owner, due: Timestamp, action: Action },
    Cancel { id: String },
    List { owner: Option<Owner> },
}
