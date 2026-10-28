//! Recoverable goal work. A pending effect is part of the accepted account revision.

use crate::turn_evidence::StopReason;
use crate::{MachineName, NoticeId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runtime {
    /// Only ends observed after this activation belong to this run of the goal.
    pub activated_at: Option<Timestamp>,
    /// The operation that opened this run, independent of clock resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<String>,
    pub settled_turn: Option<String>,
    pub pending: Option<Work>,
    pub retry: Option<Retry>,
    pub advice_sent: u8,
    /// Stops in a row of a standing goal with nothing left to wake it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unwoken: u8,
    pub last_review: Option<Review>,
    pub reviewed_through: Option<Timestamp>,
    /// The accepted review whose evidence fingerprints define the next delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_work: Option<NoticeId>,
    pub waiting: Vec<WaitKey>,
    /// The failed turn whose compaction succeeded, until the next work turn ends.
    pub compacted_after: Option<String>,
    /// The latest owner input already used to reset consecutive advice.
    pub owner_input: Option<String>,
    /// The local stop explicitly observed by a later owner resume, never an earlier in-flight resume.
    pub clears_stop: Option<String>,
    /// The first accepted conclusion of this unchanged outcome, retained across later metadata edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<Conclusion>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conclusion {
    pub operation: String,
    pub at: Timestamp,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitKey {
    pub machine: MachineName,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::turn_evidence::WaitKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<NoticeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Work {
    /// Stable across replay, including when a delivery succeeds before its receipt is saved.
    pub id: NoticeId,
    pub due: Timestamp,
    pub turn: Option<String>,
    pub task: Task,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "task", rename_all = "snake_case")]
pub enum Task {
    Idle { generation: String },
    Remind { since: Timestamp },
    Retry { stopped: StopReason },
    Quota { source: QuotaSource },
    Compact,
    Review { cause: ReviewCause, failures: u8, unavailable: Option<String> },
    Tell { text: String, advice: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaSource {
    Host,
    Assistance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retry {
    pub failures: u32,
    pub online_ms: u64,
    pub first_failure_at: Timestamp,
    /// A node activation's identity; monotonic readings from different activations cannot be subtracted.
    pub epoch: String,
    pub checkpoint_ms: u64,
    pub last: StopReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCause {
    Idle,
    Blocked,
    Paused,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub at: Timestamp,
    pub cause: ReviewCause,
    pub verdict: Verdict,
    pub delivered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Sound { reason: String },
    Advice { looked: String, saw: String, thinks: String },
    Failed { reason: String },
}

fn is_zero(count: &u8) -> bool {
    *count == 0
}
