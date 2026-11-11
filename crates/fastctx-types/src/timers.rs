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
    Tell {
        resident: ResidentId,
        text: String,
    },
    /// A sentinel's wake-up, handed to the sentinel runtime until it acknowledges it.
    Sentinel {
        action: crate::sentinel::TimerAction,
    },
    Goal {
        resident: ResidentId,
        goal: String,
        work: NoticeId,
    },
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
    Set {
        id: String,
        owner: Owner,
        due: Timestamp,
        action: Action,
    },
    Cancel {
        id: String,
    },
    List {
        owner: Option<Owner>,
        /// Continue after the last inspected timer identity.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
    },
}

impl TimerCall {
    /// Rejects oversized request fields before looking up or changing a timer.
    pub fn validate(&self) -> Result<(), String> {
        let id = match self {
            Self::Set { id, action, owner, .. } => {
                if let Action::Tell { text, .. } = action
                    && text.len() > 16 * 1024
                {
                    return Err("timer text exceeds 16384 bytes".into());
                }
                if let Owner::Sentinel { id } = owner
                    && id.len() > 256
                {
                    return Err("sentinel timer owner exceeds 256 bytes".into());
                }
                Some(id)
            }
            Self::Cancel { id } => Some(id),
            Self::List { after, owner } => {
                if let Some(Owner::Sentinel { id }) = owner.as_ref()
                    && id.len() > 256
                {
                    return Err("sentinel timer owner exceeds 256 bytes".into());
                }
                after.as_ref()
            }
        };
        if id.is_some_and(|id| id.len() > 4096) {
            return Err("timer identity or cursor exceeds 4096 bytes".into());
        }
        if matches!(self, Self::Set { id, .. } if id.is_empty()) {
            return Err("a timer needs an id".into());
        }
        Ok(())
    }
}
