//! A login's request ledger, with conversation identities removed for other readers.
use crate::ResidentId;
use crate::access::HubAccount;
use crate::usage::{Tokens, Usd};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallSource {
    Main,
    Agent,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Completed,
    Failed,
    Interrupted,
    Held,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallPhase {
    BeforeResponse,
    Streaming,
    Finished,
    Unknown,
}

/// Observed host boundaries, never inferred from token counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallTiming {
    pub started_at: Option<Timestamp>,
    pub first_at: Option<Timestamp>,
    pub ended_at: Timestamp,
    pub approximate: bool,
}

/// An opaque tie-breaker prevents identical timestamps from losing rows on a page boundary.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CallCursor {
    pub at: Timestamp,
    pub key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCallsQuery {
    #[serde(default)]
    pub person: Option<CallPerson>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub source: Option<CallSource>,
    #[serde(default)]
    pub outcome: Option<CallOutcome>,
    #[serde(default)]
    pub conversation: Option<ResidentId>,
    #[serde(default)]
    pub turn: Option<String>,
    #[serde(default)]
    pub before: Option<CallCursor>,
}

/// `account: null` selects use by the machine itself; no person filter selects everyone visible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallPerson {
    pub account: Option<HubAccount>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCall {
    pub cursor: CallCursor,
    pub who: Option<HubAccount>,
    /// Claude's unobserved auxiliary requests have neither model nor tokens.
    pub model: Option<String>,
    pub tokens: Option<Tokens>,
    pub usd: Option<Usd>,
    pub source: CallSource,
    pub agent: Option<String>,
    pub timing: CallTiming,
    pub outcome: CallOutcome,
    pub phase: CallPhase,
    /// Both identities are absent for another person's conversation, including to administrators.
    pub conversation: Option<ResidentId>,
    pub turn: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCallsPage {
    pub rows: Vec<UsageCall>,
    pub next: Option<CallCursor>,
}
