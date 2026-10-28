//! An account's goal for one conversation and the authority behind its changes.

use crate::access::HubAccount;
use crate::{Address, MachineName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Complete,
    Blocked,
    Paused,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Owner {
    User,
    Conversation { address: Address },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
    User { machine: MachineName },
    Resident { address: Address },
    World,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    User,
    Claimed,
    Quota,
    Assistance,
    Review,
    RetryExhausted,
    Context,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    pub kind: StopKind,
    pub reason: String,
    pub resume_at: Option<Timestamp>,
}

/// A main agent's blocked or paused claim still awaits its stopped-turn review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub status: Status,
    pub turn: Option<String>,
    pub at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub operation: String,
    pub at: Timestamp,
    pub by: Author,
    pub kind: String,
    pub turn: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retirement {
    Archived,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retired {
    pub at: Timestamp,
    pub why: Retirement,
}

/// The body of the `goal` account record, keyed by the conversation's resident ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    /// A new goal after clearing has a new identity; old timers cannot affect it.
    pub id: String,
    pub resident: Address,
    pub account: HubAccount,
    pub objective: String,
    pub owner: Owner,
    pub status: Status,
    /// A standing goal never completes: it is kept going, waking on what it
    /// leaves to wake it (`world-goal.md`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub standing: bool,
    /// For a colleague's goal, the goal of the conversation that asked it,
    /// as it was when it asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub created_at: Timestamp,
    pub created_by: Author,
    pub changed_at: Timestamp,
    pub stop: Option<Stop>,
    pub statement: Option<String>,
    pub claim: Option<Claim>,
    pub retired: Option<Retired>,
    pub decisions: Vec<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryRef>,
    #[serde(default)]
    pub runtime: runtime::Runtime,
    /// Identifies the last durable mutation across a lost CAS response.
    pub operation: String,
}

impl Goal {
    pub fn active(&self) -> bool {
        self.status == Status::Active && self.retired.is_none()
    }
}

/// The newest archived page. Each page links to the preceding, older decisions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRef {
    pub key: String,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryPage {
    pub goal: String,
    pub resident: crate::ResidentId,
    pub previous: Option<HistoryRef>,
    pub decisions: Vec<Decision>,
}

/// A deleted record retains its version, so a stale first write cannot resurrect it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub account: HubAccount,
    pub version: Option<u64>,
    pub goal: Option<Goal>,
    /// A locally observed stop awaiting its account record, never presented as an accepted revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
}

impl View {
    pub fn active(&self) -> bool {
        self.pending.is_none() && self.goal.as_ref().is_some_and(Goal::active)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingAction {
    Pause,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub action: PendingAction,
    pub reason: String,
    pub at: Timestamp,
}

/// A goal no longer current: done, or put down when the user cleared it or
/// set another (`world-goal.md`, 目标的历史).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PastGoal {
    pub id: String,
    pub objective: String,
    pub outcome: PastOutcome,
    pub created_at: Timestamp,
    pub closed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub standing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PastOutcome {
    Complete,
    Dropped,
}

/// The body of the `goal_past` account record, keyed as the goal's: the
/// conversation's earlier goals, oldest first, the latest few kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalPast {
    /// The conversation whose goals these were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resident: Option<Address>,
    pub goals: Vec<PastGoal>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum UserChange {
    Open {
        objective: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        standing: bool,
    },
    Edit {
        objective: String,
    },
    Pause,
    Resume,
    Clear,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mark", rename_all = "snake_case")]
pub enum Mark {
    Complete { statement: String },
    Blocked { statement: String },
    Pause,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    Read,
    History {
        page: Option<String>,
    },
    /// The conversation's earlier goals.
    Past,
    Change {
        operation: String,
        expected: Option<u64>,
        change: UserChange,
    },
    Mark {
        operation: String,
        mark: Mark,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Updated {
    Goal { view: Box<View> },
    History { page: Option<Box<HistoryPage>> },
    Past { past: GoalPast },
    Conflict { current: Option<u64> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum AgentRequest {
    Read,
    Mark { operation: String, mark: Mark },
    ControlColleague { operation: String, colleague: crate::ResidentId, control: ColleagueControl },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColleagueControl {
    Pause,
    Resume,
}

impl AgentRequest {
    /// The conversation whose goal this command changes or reads.
    pub fn resident<'a>(&'a self, actor: &'a crate::Address) -> &'a crate::ResidentId {
        match self {
            Self::ControlColleague { colleague, .. } => colleague,
            Self::Read | Self::Mark { .. } => &actor.resident,
        }
    }
}

/// A settled goal's immutable report to the conversation that requested it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub id: crate::NoticeId,
    pub goal: String,
    pub colleague: crate::Address,
    pub account: HubAccount,
    pub label: String,
    pub status: Status,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    pub resume_at: Option<Timestamp>,
    pub text: String,
}

/// Metadata on an ordinary resident message. Older readers retain its sender and full text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportDelivery {
    pub id: crate::NoticeId,
    /// The goal run fulfilled by a final result; quota notices do not fulfill it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<crate::NoticeId>,
    pub colleague: crate::Address,
    pub account: HubAccount,
    /// The report as an interface draws it; older senders leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<ReportShown>,
}

/// A colleague's report as an interface draws it, beside the words the
/// requester's model reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportShown {
    pub label: String,
    pub status: Status,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_at: Option<Timestamp>,
    /// The colleague's conclusion, as it wrote it.
    pub text: String,
}

/// A nonblocking question attached to ordinary resident words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionDelivery {
    pub id: String,
    pub colleague: crate::Address,
    /// Older questions and colleagues without a goal did not retain a goal account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<HubAccount>,
    /// The questions as the colleague's host asked them, for an interface
    /// to draw; older senders leave them out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub questions: Option<serde_json::Value>,
}

/// A short entry for `w`; the report file retains the complete immutable text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSummary {
    pub id: crate::NoticeId,
    pub account: HubAccount,
    pub status: Status,
    pub ended_at: Timestamp,
    pub elapsed_seconds: u64,
    pub preview: String,
    pub path: String,
}

pub mod runtime;
pub mod words;
