//! Facts obtained from the programs this machine actually uses to start agents.

use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{AgentName, MachineName};

/// A measured answer, an unavailable official interface, or a failed query.
/// Missing information must never be interpreted as a negative answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Fact<T> {
    Known { value: T },
    Unavailable { reason: String },
    Failed { reason: String },
}

impl<T> Fact<T> {
    pub fn known(value: T) -> Self {
        Self::Known { value }
    }
    pub fn failed(reason: impl Into<String>) -> Self {
        Self::Failed { reason: reason.into() }
    }
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable { reason: reason.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldAgents {
    pub machine: MachineName,
    pub machines: Vec<MachineAgents>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MachineAgents {
    Answered { reply: AgentsReply },
    Offline { machine: MachineName, since: Option<Timestamp> },
    NoAnswer { machine: MachineName, reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsReply {
    pub machine: MachineName,
    pub checked_at: Timestamp,
    pub elapsed_ms: u64,
    pub agents: Vec<AgentFacts>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFacts {
    pub agent: AgentName,
    /// None means neither an app-configured program nor a PATH candidate
    /// was found. A stale app path remains visible with `exists: false`.
    pub program: Fact<Option<AgentProgram>>,
    pub version: Fact<String>,
    pub launch: Fact<LaunchCheck>,
    /// Only the official status command's answer, never an inference from
    /// a directory, a credential file, or the model catalog.
    pub login: Fact<LoginStatus>,
    pub models: Fact<Vec<AgentModel>>,
    /// The local apply receipt, not a claim about already-open sessions.
    pub integration: Fact<Integration>,
    pub timings: Vec<ProbeTime>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProgram {
    pub path: PathBuf,
    pub source: ProgramSource,
    pub exists: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgramSource {
    DesktopApp,
    Path,
}

/// Successful preflight evidence. Neither check creates a session or
/// verifies model entitlement, inference, or a session's tool startup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LaunchCheck {
    /// The node's real app-server completed its handshake. Its running
    /// program can differ from the configured one until busy turns finish.
    CodexAppServer { program: PathBuf, pid: u32 },
    /// The executable ran and its help advertised --bg. Claude has no
    /// public dry run for creating a background session.
    ClaudeCommand,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LoginStatus {
    LoggedIn { method: Option<String> },
    LoggedOut,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModel {
    pub id: String,
    /// The exact value accepted by Open's model field.
    pub model: String,
    pub display_name: String,
    pub description: String,
    pub is_default: bool,
    pub default_reasoning_effort: String,
    pub reasoning_efforts: Vec<ReasoningEffort>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningEffort {
    pub effort: String,
    pub description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integration {
    Absent,
    Out,
    In,
    Old,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeTime {
    pub step: String,
    pub elapsed_ms: u64,
}

/// Omitted fields keep the official program's configured defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOptions {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}
