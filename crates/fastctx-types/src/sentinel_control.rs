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
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HomeReply {
    Judged { judgment: crate::sentinel::Judgment },
    Summary { sentinel: Box<Summary> },
    Withdrawn,
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
