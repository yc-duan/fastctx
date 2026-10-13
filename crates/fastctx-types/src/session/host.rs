//! Facts supplied by a host's model requests and native agent roster.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAgent {
    pub id: String,
    pub description: String,
    #[serde(rename = "type")]
    pub agent_type: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teammate_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub detail: std::collections::BTreeMap<String, Value>,
}

/// One of the host's own agents in a conversation: Claude Code's subagent,
/// a thread Codex spawned. Each telling adds what it knows of the agent;
/// none takes back what another told.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAgentStart {
    /// The host's id for it: Claude Code's agent id, Codex's thread id.
    pub id: String,
    /// The tool call that started it, in the loop that did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// The agent whose loop started it; none when the conversation's own
    /// loop did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Its kind as the host names it: Claude Code's agent type, Codex's role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// What it goes by: Claude Code's agent name, Codex's nickname.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The few words of its task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// What it was asked, whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// It goes on after the call that started it has answered.
    #[serde(default)]
    pub background: bool,
    /// Told as it started, by what watched it start: what it does from
    /// then on comes as it happens (`SessionChange::AgentEvent`). Read from
    /// the host's record otherwise.
    #[serde(default)]
    pub live: bool,
}

/// How one of the host's own agents ended its latest run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAgentEnd {
    pub how: super::TurnHow,
    /// Why it did not complete, in the host's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// The tokens it used, as its host counted them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    /// How many tools it called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<u64>,
    /// Its last words: what it handed back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// The few words of its task, for a notice read apart from its start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The conversation was told of it here, by the host's own notice,
    /// which the conversation's loop then took up.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub told: bool,
}

/// What one of the host's own agents did, as its host keeps it: its own
/// events, as a conversation of its own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentActivity {
    /// What its own record tells of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<HostAgentStart>,
    pub events: Vec<super::SessionEvent>,
    /// Why its record could not be read, where it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}
