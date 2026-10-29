//! Conversation-owned sentinel lifecycle calls and their public summaries.
use crate::{
    Address, SentinelId,
    access::HubAccount,
    sentinel::{Definition, Kind, Observation},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Create {
    pub condition: String,
    pub at: Vec<i64>,
    pub web: bool,
    pub tell: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    Create { request: Create },
    List { owner: Option<Address> },
    Stop { id: SentinelId, home: Option<crate::MachineName> },
    Help,
    /// What it did, read as a conversation, from the machine it lives on.
    Record { id: SentinelId, home: crate::MachineName, generation: u64, held: Option<String> },
    /// Every sentinel of the account, ended or not, with when each woke and
    /// what each wake took, from the machines they live on.
    Overview,
}
/// The account record owns assignment and lifetime; detailed state stays at home.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    pub scope: HubAccount,
    pub definition: Definition,
    pub active: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Watching,
    Satisfied,
    Unable,
    Withdrawn,
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub definition: Definition,
    pub phase: Phase,
    pub plan: String,
    pub latest: Option<Observation>,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    Created { sentinel: Box<Summary> },
    Listed { sentinels: Vec<Summary>, unavailable: Vec<String> },
    Stopped { id: SentinelId, home_confirmed: bool, reason: Option<String> },
    Help { text: String },
    Record { record: Box<Record> },
    Overview { watched: Vec<Watched>, unavailable: Vec<String> },
}

/// One sentinel as its home keeps it, ended or not, with its wakes
/// (`world-senses.md`, 全部观测).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watched {
    pub summary: Summary,
    /// Its wakes, oldest first, the latest of the last days.
    pub wakes: Vec<Wake>,
    /// It woke earlier too: those wakes are left out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub earlier: bool,
}

/// One time a sentinel woke, and what it took.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wake {
    /// In milliseconds since the epoch.
    pub at: i64,
    pub why: crate::residents::Woke,
    /// Tokens: what it read, cached ones included, what it wrote, and what
    /// of its reading came from the cache.
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub requests: u32,
    pub elapsed_ms: u64,
    /// It ended in an error.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
}

/// What a sentinel did, as a conversation reads (`world-senses.md`, 看它做了什么):
/// each time it woke a turn, opened by why it woke, holding its thinking,
/// its words, where it looked and how, and what it said of what it watches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Names what was read; read again unchanged, it is the same.
    pub version: String,
    /// The same as the version the reader held: no events are sent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unchanged: bool,
    /// Its latest wakes, oldest first.
    pub events: Vec<crate::session::SessionEvent>,
    /// It woke more often than is sent: the earliest wakes are left out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub earlier: bool,
    /// How it stands as its home last kept it, ended or not; none before
    /// its home first kept anything of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Box<Summary>>,
    /// When it next looks or is woken, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<i64>,
}
impl Create {
    pub fn validate(&self, now: i64) -> Result<(), String> {
        if self.condition.trim().is_empty() && self.at.is_empty() {
            return Err("name a condition or an alarm time".into());
        }
        if self.condition.len() > 4096 || self.tell.len() > 4096 || self.at.len() > 32 {
            return Err("sentinel definition is too large".into());
        }
        if self.at.iter().any(|at| *at <= now) {
            return Err("alarm times must be in the future".into());
        }
        Ok(())
    }
    pub fn kind(&self) -> Kind {
        if self.web { Kind::Web } else { Kind::Local }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HomeRequest {
    Begin { id: SentinelId, version: u64, owner: Address },
    Inspect { id: SentinelId },
    Judge { request: Box<GoalRequest> },
    Withdraw { id: SentinelId },
    Record { id: SentinelId, generation: u64, held: Option<String> },
    Overview,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HomeReply {
    Judged { judgment: crate::sentinel::Judgment },
    Summary { sentinel: Box<Summary> },
    Withdrawn,
    Record { record: Box<Record> },
    Overview { watched: Vec<Watched> },
}

/// A stable engine request ID is scoped by the sentinel generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalRequest {
    pub id: SentinelId,
    pub generation: u64,
    pub owner: Address,
    pub request_id: String,
    pub review: crate::sentinel::GoalReview,
}
