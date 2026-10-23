//! Model collaboration belongs to an account, not to a project or session;
//! a computer in no network keeps its own (`world-colleagues.md`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{AgentName, MachineName};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLogin {
    pub machine: MachineName,
    pub agent: AgentName,
    pub home: Option<PathBuf>,
}

/// A local tool adapter's agent and login; its node supplies the machine identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    pub agent: AgentName,
    pub home: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Callee {
    pub name: String,
    pub use_when: String,
    pub login: AgentLogin,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default = "screen_default")]
    pub screen: bool,
}

fn screen_default() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collab {
    pub version: u64,
    pub on: bool,
    pub callers: Vec<AgentLogin>,
    pub callees: Vec<Callee>,
    pub prompt: Option<String>,
    pub rules: String,
}

impl Collab {
    pub fn validate(&self) -> Result<(), String> {
        let mut names = std::collections::HashSet::new();
        for callee in &self.callees {
            if callee.name.is_empty()
                || !callee.name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            {
                return Err(format!("callee name {:?} must contain lowercase letters, digits or hyphens", callee.name));
            }
            if !names.insert(&callee.name) {
                return Err(format!("callee name {} is repeated in this account", callee.name));
            }
            if callee.login.agent.as_str() != "codex" {
                return Err(format!("{} must use a Codex login", callee.name));
            }
        }
        for caller in &self.callers {
            if !matches!(caller.agent.as_str(), "codex" | "claude") {
                return Err(format!("{} is not a Codex or Claude login", caller.agent));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollabView {
    pub collab: Collab,
    pub default_prompt: String,
}

/// The caller's tool projection; account configuration remains on the node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offered {
    pub callees: Vec<Offer>,
    pub prompt: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub name: String,
    /// The account it works for; none for a callee of a computer in no
    /// network, which works for that computer's user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub use_when: String,
    pub lives_on: Option<String>,
}

/// The goal runtime's summary for a colleague shown in the collaboration tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalSummary {
    pub state: GoalState,
    pub started_at: jiff::Timestamp,
    pub elapsed_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    Active,
    Complete,
    Blocked,
    Paused,
}

/// Kept with a callee so cold resumes retain the configuration it was born with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub callee: Callee,
    pub rules: String,
}
