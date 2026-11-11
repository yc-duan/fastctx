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
        /// Replaces an existing row while retaining its identity.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handle: Option<NoticeId>,
    },
    Cancel {
        #[serde(default)]
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handle: Option<NoticeId>,
    },
    List {
        owner: Option<Owner>,
        /// Opaque continuation after the last inspected row. Empty pages can have a continuation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
        /// The caller understands continuation and incomplete-page notices.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        paged: bool,
    },
}

impl TimerCall {
    /// Requires the node's `Welcome.timer_pages` capability. Older nodes ignore
    /// these fields and cannot safely interpret handle-based mutations.
    pub fn needs_pages(&self) -> bool {
        matches!(
            self,
            Self::Set { handle: Some(_), .. }
                | Self::Cancel { handle: Some(_), .. }
                | Self::List { paged: true, .. }
                | Self::List { after: Some(_), .. }
        )
    }

    /// Bounds routing and new text before lookup. Legacy identities are point-read
    /// before the creation-only identity limit is applied.
    pub fn validate(&self) -> Result<(), String> {
        if matches!(self, Self::Set { id, handle: Some(_), .. } | Self::Cancel { id, handle: Some(_) } if !id.is_empty())
        {
            return Err("a timer handle cannot be combined with an identity".into());
        }
        let owner = match self {
            Self::Set { owner, .. } => Some(owner),
            Self::List { owner, .. } => owner.as_ref(),
            _ => None,
        };
        if owner.and_then(Owner::conversation).is_some_and(|resident| resident.len() > 65408) {
            return Err("timer conversation exceeds 65408 bytes".into());
        }
        if let Self::Set { action: Action::Tell { resident, .. }, .. } = self
            && resident.len() > 65408
        {
            return Err("timer conversation exceeds 65408 bytes".into());
        }
        let id = match self {
            Self::Set { action, owner, .. } => {
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
                None
            }
            Self::Cancel { .. } => None,
            Self::List { after, owner, .. } => {
                if let Some(Owner::Sentinel { id }) = owner.as_ref()
                    && id.len() > 256
                {
                    return Err("sentinel timer owner exceeds 256 bytes".into());
                }
                after.as_ref()
            }
        };
        if id.is_some_and(|id| id.len() > 65536) {
            return Err("timer cursor exceeds 65536 bytes".into());
        }
        if matches!(self, Self::Set { id, handle: None, .. } if id.is_empty()) {
            return Err("a timer needs an id".into());
        }
        Ok(())
    }
}

/// A row that remains addressable even when its legacy identity or payload is oversized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OversizedTimer {
    pub handle: NoticeId,
    pub identity_preview: String,
    pub identity_bytes: u64,
    pub body_bytes: u64,
}
