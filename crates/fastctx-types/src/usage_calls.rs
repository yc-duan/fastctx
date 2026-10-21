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
    /// Rows that ended at or after this moment.
    #[serde(default)]
    pub from: Option<Timestamp>,
    /// Rows that ended before this moment.
    #[serde(default)]
    pub until: Option<Timestamp>,
    /// The page asked for by its number, from 0: the answer then counts
    /// every row the query selects. A query without it pages by `before`.
    #[serde(default)]
    pub page: Option<u32>,
}

/// The rows a page holds.
pub const CALLS_PAGE: usize = 100;

/// `account: null` selects use by the machine itself; no person filter selects everyone visible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallPerson {
    pub account: Option<HubAccount>,
    /// The machine's own use counted as this person's too.
    #[serde(default)]
    pub with_machine: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCall {
    pub cursor: CallCursor,
    pub who: Option<HubAccount>,
    /// Claude's unobserved auxiliary requests have neither model nor tokens.
    pub model: Option<String>,
    /// The reasoning effort the request was sent with, as its host names it;
    /// absent where the host sent none or a ledger row predates it.
    #[serde(default)]
    pub effort: Option<String>,
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
    /// Every row the query selects, where it asked for a page by number.
    #[serde(default)]
    pub total: Option<u64>,
}
