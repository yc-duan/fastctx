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
    Answered {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observation: Option<crate::observation::Observation>,
        reply: AgentsReply,
    },
    Offline {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observation: Option<crate::observation::Observation>,
        machine: MachineName,
        since: Option<Timestamp>,
    },
    NoAnswer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observation: Option<crate::observation::Observation>,
        machine: MachineName,
        reason: String,
    },
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
    /// The home these facts are of; `None` from a machine that knew one
    /// home per agent, which was its default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<AgentHome>,
    /// Who is logged in there, as the agent itself says.
    #[serde(default = "unreported")]
    pub account: Fact<AgentAccount>,
    /// Its usage limits, as the agent last read them on this machine.
    #[serde(default = "unreported")]
    pub limits: Fact<AgentLimits>,
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
    /// Whether the World can start this agent here without approvals.
    /// Claude Code needs the user's one-time consent to its
    /// bypass-permissions mode on this machine; Codex needs none.
    #[serde(default = "unreported")]
    pub unattended: Fact<bool>,
    pub timings: Vec<ProbeTime>,
    /// What bringing the World's tools here up to date waits for, if
    /// anything (`crate::logins::Drift`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift: Option<crate::logins::Drift>,
}

fn unreported<T>() -> Fact<T> {
    Fact::unavailable("this machine's World does not report it")
}

/// Where an agent keeps one login and the settings, sessions and plugins
/// that go with it: Codex's `CODEX_HOME`, Claude Code's `CLAUDE_CONFIG_DIR`.
/// A machine may hold several of each, one account apiece; neither agent
/// lists them, so the World finds them (`world-gui.md`, 账号、机器与授权).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHome {
    /// The directory, as this machine writes it.
    pub dir: PathBuf,
    pub found: HomeFound,
}

/// How the World knows of a home.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeFound {
    /// The one the agent uses when nothing names another: `CODEX_HOME` or
    /// `CLAUDE_CONFIG_DIR` in the user's environment, else `~/.codex` or
    /// `~/.claude`.
    Default,
    /// A directory of the user's home that holds that agent's things.
    Found,
    /// One the user added.
    Added,
}

/// What the user changes of the homes the World knows on a machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum HomeChange {
    /// A directory the World did not find, as that machine writes it.
    Add { agent: AgentName, dir: PathBuf },
    /// One the World found or the user added, no longer listed or used.
    Hide { agent: AgentName, dir: PathBuf },
    /// One hidden before, listed again.
    Show { agent: AgentName, dir: PathBuf },
}

/// Who is logged in to a home, as far as the agent says without anyone
/// reading its credentials: an API key or a long-lived token names no one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAccount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    /// The subscription, as the agent names it: `pro`, `max`, ….
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// An account's usage limits, as its agent read them on its own machine, and
/// when: neither agent says how fresh its numbers are, and Claude Code reads
/// them only from the answers of a session at work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLimits {
    pub observed_at: Timestamp,
    /// No window is no reading, never 0% used.
    pub windows: Vec<LimitWindow>,
    /// What the account can spend once a window is used up, when it has
    /// any: a spent window then does not stop it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<AgentCredits>,
}

/// An account's credits, as its agent says them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCredits {
    /// The balance as the agent writes it; none when it does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
    pub unlimited: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitWindow {
    /// The agent's own name for it: Claude Code's `five_hour`, `seven_day`,
    /// `spend_limit`; Codex's limit's name or id.
    pub name: String,
    /// How long the window is, when the agent says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u32>,
    /// How much of it is used, in tenths of a percent; it can pass 1000.
    pub used_tenths: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
    /// When its usage began counting this period, where that was not a
    /// whole window before it resets: the vendor cleared it in place, as
    /// when the plan changed, its reset kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Timestamp>,
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
    /// The context window it works in unless told otherwise, in tokens,
    /// where its host says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// The most its context window may be set to, where its host says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_window: Option<u64>,
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

/// What the user chose for a conversation they open, beyond its agent,
/// project and model (`world-gui.md`, 新对话).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choices {
    /// The account it works for, where this computer is logged in to more
    /// than one that may open it there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<crate::access::HubAccount>,
    /// The machines and folders the user points it to.
    #[serde(default, skip_serializing_if = "crate::focus::Focus::is_empty")]
    pub focus: crate::focus::Focus,
    /// The agent's home it uses on the machine it lives on, when not the
    /// default one there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<PathBuf>,
    /// Legacy destination hint, retained for older clients. Originals now
    /// stay on the sending computer, or at the conversation's home for a phone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_on: Option<crate::access::Written>,
    /// The account's MCP servers it starts without (`world-user-config.md`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub mcp_off: std::collections::BTreeSet<String>,
}

/// Omitted fields keep the official program's configured defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOptions {
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub mcp_off: std::collections::BTreeSet<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}
