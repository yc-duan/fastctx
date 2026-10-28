//! Collaboration as a conversation's colleagues and their native agent trees.

use crate::{Address, collab::GoalSummary, residents::Now};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tree {
    pub conversation: Branch,
    pub colleagues: Vec<Branch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub address: Address,
    pub label: Option<String>,
    pub born_at: Option<Timestamp>,
    pub now: Option<Now>,
    pub goal: Result<Option<GoalSummary>, String>,
    pub plan: Option<PlanAccess>,
    pub agents: Vec<NativeAgent>,
    pub unavailable: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanAccess {
    pub owner: Address,
    pub read_only: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeAgent {
    pub id: String,
    pub name: Option<String>,
    pub task: Option<String>,
    pub model: Option<String>,
    /// The host's state, absent when only a historical start was observed.
    pub state: Option<String>,
    /// An actual host start, not the time its parent learned about it.
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub duration_ms: Option<u64>,
    pub children: Vec<NativeAgent>,
    pub unavailable: Option<String>,
}
