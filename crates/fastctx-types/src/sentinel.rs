//! Sentinel definitions, bounded evidence and judgments crossing node boundaries.
use crate::{AccountName, Address, HubName, MachineName, SentinelId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Local,
    Web,
    Goal,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Definition {
    pub id: SentinelId,
    pub hub: HubName,
    pub account: AccountName,
    pub owner: Address,
    pub home: MachineName,
    pub generation: u64,
    pub condition: String,
    /// Unix milliseconds. Owner alarms are independent of reflex wake times.
    pub at: Vec<i64>,
    pub kind: Kind,
    pub tell: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Probe {
    Look { machine: MachineName, command: String },
    Web { url: String },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Predicate {
    Contains { text: String },
    Number { prefix: String, comparison: Comparison, value: f64 },
    Changed,
    ExitNonzero,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Reflex {
    pub id: String,
    pub at: i64,
    pub every_ms: Option<u64>,
    pub probe: Option<Probe>,
    pub predicate: Option<Predicate>,
    pub last_fingerprint: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub at: i64,
    pub source: String,
    pub text: String,
    pub exit_code: Option<i32>,
    pub full_path: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WakeReason {
    Initial,
    Reflex { id: String, observation: Observation },
    Time { id: String, due: i64 },
    Broken { id: String, reason: String },
    Withdrawn,
    Goal { review: Box<GoalReview> },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Conclusion {
    Watching,
    Satisfied { reason: String, evidence: Vec<Observation> },
    Unable { reason: String },
    Withdrawn,
    Goal { judgment: Judgment },
    NotJudged { reason: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Judgment {
    Sound { reason: String },
    Advice { looked: String, saw: String, thinks: String },
    NotJudged { reason: String },
    Unavailable { reason: String },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalTrigger {
    Idle,
    Blocked,
    Paused,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalReview {
    pub trigger: GoalTrigger,
    pub suggestions_left: u8,
    pub goal: String,
    pub plan_claim: Option<String>,
    pub actions: Vec<ActionFact>,
    pub turn_end: Option<String>,
    pub last_words_claim: Option<String>,
    pub blocked_claim: Option<String>,
    pub user_words: Vec<String>,
    pub previous_judgments: Vec<Judgment>,
    pub machines: Vec<MachineName>,
    pub now: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionFact {
    pub at: i64,
    pub machine: MachineName,
    pub summary: String,
    pub exit_code: Option<i32>,
    pub full_path: Option<String>,
}
/// What a sentinel's durable timer does when it fires; only the sentinel runtime acts on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimerAction {
    WakeSentinel {
        scope: crate::access::HubAccount,
        id: SentinelId,
        generation: u64,
        wake: ScheduledWake,
    },
    RunReflex {
        scope: crate::access::HubAccount,
        id: SentinelId,
        generation: u64,
        reflex: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduledWake {
    OwnerAlarm { index: usize },
    SentinelTime { reflex: String },
}
