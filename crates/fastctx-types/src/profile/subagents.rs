//! The account's Claude agent definitions. Login directories are never involved.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// Only fields that describe the agent itself, never installation capabilities.
pub const CLAUDE_SUBAGENT_FIELDS: &[&str] = &[
    "name",
    "description",
    "prompt",
    "tools",
    "disallowedTools",
    "model",
    "effort",
    "maxTurns",
    "initialPrompt",
    "background",
    "omitClaudeMd",
    "color",
];
pub const CLAUDE_SUBAGENT_COLORS: &[&str] = &["red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subagents {
    pub agents: Vec<Map<String, Value>>,
    pub instructions: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum SubagentOperation {
    List,
    /// None creates a new name; Some renames or replaces exactly that name.
    Save {
        version: u64,
        previous: Option<String>,
        agent: Map<String, Value>,
    },
    Delete {
        version: u64,
        name: String,
    },
    Instructions {
        version: u64,
        text: String,
    },
    Reset {
        version: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentView {
    pub hub: crate::HubName,
    pub account: crate::AccountName,
    pub supported: bool,
    pub version: u64,
    pub configured: bool,
    /// Defaults are materialized here, never in a redacted profile.
    pub configuration: Subagents,
}

impl Default for Subagents {
    fn default() -> Self {
        let agents = [
            include_str!("agent-defaults/Explore.md"),
            include_str!("agent-defaults/Plan.md"),
            include_str!("agent-defaults/general-purpose.md"),
        ]
        .into_iter()
        .map(bundled)
        .collect();
        Self { agents, instructions: include_str!("agent-defaults/instructions.md").into() }
    }
}

// The three bundled files have flat, string-valued headers. This is not a YAML importer.
fn bundled(text: &str) -> Map<String, Value> {
    let text = text.strip_prefix("---\n").expect("bundled frontmatter");
    let (header, prompt) = text.split_once("\n---\n").expect("bundled frontmatter end");
    let mut spec = Map::new();
    for line in header.lines() {
        let (key, value) = line.split_once(':').expect("bundled field");
        let value = value.trim();
        let value = if matches!(key, "tools" | "disallowedTools") {
            Value::Array(value.split(',').map(|v| Value::String(v.trim().into())).collect())
        } else {
            Value::String(value.into())
        };
        spec.insert(key.into(), value);
    }
    spec.insert("prompt".into(), prompt.into());
    spec
}

pub fn validate_subagent(spec: &Map<String, Value>) -> Result<(), String> {
    for key in spec.keys() {
        if !CLAUDE_SUBAGENT_FIELDS.contains(&key.as_str()) {
            return Err(format!("{key} is not an allowed Claude subagent field"));
        }
    }
    let name = spec.get("name").and_then(Value::as_str).ok_or("a subagent needs a string name")?;
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('-')
        || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(
            "a subagent name needs 1–64 ASCII letters, digits, underscores or hyphens, and cannot start with a hyphen"
                .into(),
        );
    }
    for key in ["description", "prompt"] {
        if !spec.get(key).is_some_and(Value::is_string) {
            return Err(format!("subagent {name} needs a string {key}"));
        }
    }
    if spec["description"].as_str().expect("string").trim().is_empty() {
        return Err(format!("subagent {name} needs a nonempty description"));
    }
    for key in ["model", "initialPrompt", "color"] {
        if spec.get(key).is_some_and(|v| !v.is_string()) {
            return Err(format!("subagent {name}: {key} must be a string"));
        }
    }
    if spec.get("model").is_some_and(|v| v.as_str().expect("string").trim().is_empty()) {
        return Err(format!("subagent {name}: model cannot be empty"));
    }
    if let Some(color) = spec.get("color").and_then(Value::as_str)
        && !CLAUDE_SUBAGENT_COLORS.contains(&color)
    {
        return Err(format!("subagent {name}: unknown color {color}"));
    }
    for key in ["tools", "disallowedTools"] {
        if let Some(value) = spec.get(key)
            && !value.as_array().is_some_and(|v| v.iter().all(Value::is_string))
        {
            return Err(format!("subagent {name}: {key} must be an array of strings"));
        }
    }
    if let Some(value) = spec.get("effort")
        && !(value.as_str().is_some_and(|s| ["low", "medium", "high", "xhigh", "max"].contains(&s))
            || value.as_i64().is_some())
    {
        return Err(format!("subagent {name}: effort must be low, medium, high, xhigh, max or an integer"));
    }
    if let Some(value) = spec.get("maxTurns")
        && value.as_u64().is_none_or(|v| v == 0)
    {
        return Err(format!("subagent {name}: maxTurns must be a positive integer"));
    }
    for key in ["background", "omitClaudeMd"] {
        if spec.get(key).is_some_and(|v| !v.is_boolean()) {
            return Err(format!("subagent {name}: {key} must be a boolean"));
        }
    }
    Ok(())
}

impl Subagents {
    pub fn validate(&self) -> Result<(), String> {
        if self.instructions.len() > super::PROMPT_MOST {
            return Err("subagent instructions exceed 32768 bytes".into());
        }
        let mut names = BTreeSet::new();
        for spec in &self.agents {
            validate_subagent(spec)?;
            if !names.insert(spec["name"].as_str().expect("validated name")) {
                return Err(format!("duplicate subagent name: {}", spec["name"]));
            }
        }
        Ok(())
    }

    /// Independently filters a hub-provided definition before registration.
    pub fn for_session(&self) -> Result<Self, String> {
        let mut value = self.clone();
        for spec in &mut value.agents {
            spec.retain(|key, _| CLAUDE_SUBAGENT_FIELDS.contains(&key.as_str()) && key != "color");
        }
        value.validate()?;
        Ok(value)
    }

    pub fn change(&mut self, operation: &SubagentOperation) -> Result<(), String> {
        match operation {
            SubagentOperation::Save { previous, agent, .. } => {
                validate_subagent(agent)?;
                let name = agent["name"].as_str().expect("validated name");
                let at = previous
                    .as_ref()
                    .map(|previous| {
                        self.agents
                            .iter()
                            .position(|a| a["name"].as_str() == Some(previous))
                            .ok_or_else(|| format!("there is no subagent {previous}"))
                    })
                    .transpose()?;
                if self.agents.iter().enumerate().any(|(i, a)| Some(i) != at && a["name"].as_str() == Some(name)) {
                    return Err(format!("subagent {name} already exists"));
                }
                if let Some(at) = at {
                    self.agents[at] = agent.clone();
                } else {
                    self.agents.push(agent.clone());
                }
            }
            SubagentOperation::Delete { name, .. } => {
                let at = self
                    .agents
                    .iter()
                    .position(|a| a["name"].as_str() == Some(name))
                    .ok_or_else(|| format!("there is no subagent {name}"))?;
                self.agents.remove(at);
            }
            SubagentOperation::Instructions { text, .. } => self.instructions = text.clone(),
            SubagentOperation::Reset { .. } | SubagentOperation::List => {}
        }
        self.validate()
    }
}

impl SubagentOperation {
    pub fn version(&self) -> Option<u64> {
        match self {
            Self::List => None,
            Self::Save { version, .. }
            | Self::Delete { version, .. }
            | Self::Instructions { version, .. }
            | Self::Reset { version } => Some(*version),
        }
    }
}
