//! Observations of a conversation turn, distinct from the agent's own claims.

use crate::agents::{AgentLimits, Fact};
use crate::residents::Action;
use crate::run::JobRecord;
use crate::run_facts::RunFacts;
use crate::session::TurnEnd;
use crate::{MachineName, ResidentId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    Completed,
    UserStopped,
    ContextWindowExceeded,
    UsageLimited,
    HostExited,
    NodeRestarted,
    OtherError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopReason {
    pub kind: StopKind,
    pub category: Option<serde_json::Value>,
    pub message: Option<String>,
}

/// The source of an interruption requested by the node, never inferred from an error message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopSource {
    User,
    Allowance,
    Lifecycle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallEvidence {
    pub call_id: String,
    pub script: String,
    pub source: String,
    pub facts: RunFacts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobEvidence {
    pub source: String,
    pub record: JobRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
    Job,
    Colleague,
    Sentinel,
    Timer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Liveness {
    Alive,
    Ended,
    Unknown { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitHandle {
    pub id: String,
    pub kind: WaitKind,
    pub machine: MachineName,
    pub since: Option<Timestamp>,
    pub liveness: Liveness,
    /// A durable World obligation will deliver a result, even if its executor is offline now.
    pub will_wake: bool,
    /// A stable identity carried by the completion notification, when the producer can name it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<crate::NoticeId>,
    pub summary: String,
    pub output: Option<WaitOutput>,
    pub output_growth: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quota {
    pub observed_at: Timestamp,
    pub limits: Fact<AgentLimits>,
    /// Codex's account-validated permission, not a deduction from its windows.
    pub ordinary_usage_allowed: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEvidence {
    pub resident: ResidentId,
    pub turn: Option<String>,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    /// When the World first durably accepted the terminal, on its own clock.
    /// Host timestamps may have lower precision or arrive after recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_recorded_at: Option<Timestamp>,
    pub last_activity_at: Option<Timestamp>,
    pub end: Option<TurnEnd>,
    pub stop: Option<StopReason>,
    pub actions: Vec<Action>,
    pub calls: Vec<CallEvidence>,
    pub jobs: Vec<JobEvidence>,
    pub job_updates: Vec<crate::run_facts::JobFacts>,
    pub notices: Vec<crate::session::SessionEvent>,
    pub waiting: Vec<WaitHandle>,
    pub quota: Option<Quota>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Compacted {
    Done,
    Refused { reason: String },
    Failed { reason: String },
}

/// Only the selected requester's outstanding deliveries, never their message contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationDebt {
    pub observed_at: Timestamp,
    /// The selected requester's autonomous goal, separate from ordinary turn replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<Box<GoalDebt>>,
    pub pending: Vec<(String, Timestamp)>,
    pub running: Vec<(String, Timestamp)>,
    pub answers: Vec<(String, Timestamp)>,
    pub uncertain: Vec<(String, Timestamp)>,
}

/// A goal's promise to its requester, retained until its final report reaches that requester.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalDebt {
    pub account: crate::access::HubAccount,
    pub completion: crate::NoticeId,
    pub since: Timestamp,
    pub continuing: bool,
    /// The exact accepted final report, even after its source outbox has handed it to the network.
    pub report: Option<crate::NoticeId>,
    pub summary: String,
}

impl ConversationDebt {
    pub fn since(&self) -> Option<Timestamp> {
        self.pending.iter().chain(&self.running).chain(&self.answers).chain(&self.uncertain).map(|(_, at)| *at).min()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitOutput {
    /// The complete log, on the handle's machine, including its latest lines.
    pub path: String,
    pub bytes: u64,
}
