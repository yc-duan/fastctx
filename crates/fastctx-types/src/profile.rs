//! What an account keeps for the sessions that work for it, on top of the
//! agent login's own configuration (`world-user-config.md`): its prompt,
//! the settings it may set, and its MCP servers. The account's hub keeps
//! the prompt, the settings and the list of servers; of a server a command
//! starts, the list says only which machine it is on, and its command,
//! arguments and environment never leave that machine.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::MachineName;

mod subagents;
pub use subagents::{
    CLAUDE_SUBAGENT_COLORS, CLAUDE_SUBAGENT_FIELDS, SubagentOperation, SubagentView, Subagents, validate_subagent,
};

/// The account's own layer, versioned so two devices' edits never cross.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub version: u64,
    /// Whether conversations receive their project's memory map. On unless
    /// the account turned it off, so only an off is written.
    #[serde(default = "on", skip_serializing_if = "is_on")]
    pub memory: bool,
    /// What the account writes to every agent that works for it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// Claude Code settings from `CLAUDE_SETTINGS` alone.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub claude: Map<String, Value>,
    /// Codex configuration from `CODEX_SETTINGS` alone.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub codex: Map<String, Value>,
    /// Its MCP servers, by the name its sessions list them under.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, Entry>,
    /// None follows this version's defaults; Some may deliberately contain no agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Subagents>,
}

/// One server in the account's list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Entry {
    /// Reached at an address, by the host of whichever machine the session
    /// runs on.
    Remote(RemoteMcp),
    /// Started by a command on `machine`, which alone keeps the command
    /// (`CommandMcp`); sessions elsewhere reach it through the World.
    Command { machine: MachineName },
    /// A kind a later release adds: kept as it is read, never used, and
    /// never written.
    #[serde(other)]
    Unknown,
}

/// An MCP server reached at an address; it runs wherever its owner keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMcp {
    pub url: String,
    /// The older SSE transport; the streamable HTTP one otherwise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sse: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

/// An MCP server a command starts, as the machine that runs it keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandMcp {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Where it starts, an absolute path as that machine writes it; the
    /// user's home there when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// One server as an `mcpServers` entry describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Server {
    Command(CommandMcp),
    Remote(RemoteMcp),
}

/// The longest prompt an account keeps, in bytes.
pub const PROMPT_MOST: usize = 32 * 1024;

/// The most MCP servers an account keeps, of both kinds together.
pub const SERVERS_MOST: usize = 64;

/// The longest name a server may have: it is part of every tool name a
/// session lists, which the model interfaces cap.
pub const NAME_MOST: usize = 32;

/// The name the World's own tools go under in every session.
pub const RESERVED: &str = "fastctx";

/// The Claude Code settings an account may set for its own sessions: what
/// changes the model, how it works and what it writes, and when its
/// context is compacted; never what runs a program, loosens permissions,
/// changes credentials or endpoints, enables servers or plugins, or reaches
/// the whole installation. Of the environment, only `CLAUDE_ENV`.
pub const CLAUDE_SETTINGS: &[&str] = &[
    "attribution",
    "autoCompactEnabled",
    "autoCompactWindow",
    "effortLevel",
    "env",
    "includeGitInstructions",
    "language",
    "model",
    "outputStyle",
    "showThinkingSummaries",
];

/// The variables an account may set in its Claude Code settings' `env`:
/// the share of the window at which a session compacts. The node's own
/// variables are written over them.
pub const CLAUDE_ENV: &[&str] = &["CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"];

/// The Codex settings an account may set for its own threads, by the same
/// rule.
pub const CODEX_SETTINGS: &[&str] = &[
    "hide_agent_reasoning",
    "model",
    "model_auto_compact_token_limit",
    "model_context_window",
    "model_reasoning_effort",
    "model_reasoning_summary",
    "model_verbosity",
    "show_raw_agent_reasoning",
];

impl Default for Profile {
    fn default() -> Self {
        Self {
            version: 0,
            memory: on(),
            prompt: String::new(),
            claude: Map::new(),
            codex: Map::new(),
            servers: BTreeMap::new(),
            subagents: None,
        }
    }
}

fn on() -> bool {
    true
}

fn is_on(value: &bool) -> bool {
    *value
}

impl Profile {
    /// Whether this is a profile an account may keep.
    pub fn validate(&self) -> Result<(), String> {
        if self.prompt.len() > PROMPT_MOST {
            return Err(format!("the prompt is {} bytes; at most {PROMPT_MOST}", self.prompt.len()));
        }
        if let Some(subagents) = &self.subagents {
            subagents.validate()?;
        }
        settings(&self.claude, CLAUDE_SETTINGS, "Claude Code")?;
        claude_env(&self.claude)?;
        settings(&self.codex, CODEX_SETTINGS, "Codex")?;
        if self.servers.len() > SERVERS_MOST {
            return Err(format!("{} MCP servers; at most {SERVERS_MOST}", self.servers.len()));
        }
        for (name, entry) in &self.servers {
            name_of(name)?;
            match entry {
                Entry::Remote(remote) => remote.validate(name)?,
                Entry::Command { .. } => {}
                Entry::Unknown => return Err(format!("the MCP server {name} is of a kind this release cannot write")),
            }
        }
        Ok(())
    }

    /// What of it a machine that does not work for the account is given:
    /// the servers that run on it, as `here` tells its names, so it can let
    /// go of what it keeps of the others; and the version that says so.
    pub fn located(&self, here: impl Fn(&MachineName) -> bool) -> Self {
        let servers = self
            .servers
            .iter()
            .filter(|(_, entry)| matches!(entry, Entry::Command { machine } if here(machine)))
            .map(|(name, entry)| (name.clone(), entry.clone()))
            .collect();
        Self { version: self.version, servers, ..Self::default() }
    }
}

impl RemoteMcp {
    fn validate(&self, name: &str) -> Result<(), String> {
        if self.url.starts_with("https://") || self.url.starts_with("http://") {
            Ok(())
        } else {
            Err(format!("the MCP server {name} is at {}, no http or https address", self.url))
        }
    }
}

fn settings(set: &Map<String, Value>, allowed: &[&str], host: &str) -> Result<(), String> {
    match set.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => {
            Err(format!("{key} is not a {host} setting an account sets for itself; these are: {}", allowed.join(", ")))
        }
        None => Ok(()),
    }
}

/// Whether the `env` of `claude`, where there is one, sets only `CLAUDE_ENV`,
/// each to a string.
fn claude_env(claude: &Map<String, Value>) -> Result<(), String> {
    let Some(env) = claude.get("env") else { return Ok(()) };
    let Value::Object(env) = env else { return Err("env of the Claude Code settings is not a table".into()) };
    for (key, value) in env {
        if !CLAUDE_ENV.contains(&key.as_str()) {
            return Err(format!(
                "{key} is not a variable an account sets for its Claude Code sessions; these are: {}",
                CLAUDE_ENV.join(", ")
            ));
        }
        if !value.is_string() {
            return Err(format!("{key} in the Claude Code settings' env is not a string"));
        }
    }
    Ok(())
}

/// Whether `name` may name an MCP server: letters, digits, `_` and `-`, at
/// most `NAME_MOST`, and not the World's own.
pub fn name_of(name: &str) -> Result<(), String> {
    let fits = !name.is_empty()
        && name.len() <= NAME_MOST
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if !fits {
        return Err(format!("{name} cannot name an MCP server: use letters, digits, _ and -, at most {NAME_MOST}"));
    }
    if name.eq_ignore_ascii_case(RESERVED) {
        return Err(format!("{RESERVED} names the World's own tools"));
    }
    Ok(())
}

/// A name for `name` that `taken` does not hold yet: as it is, then with
/// what of `machine`'s name can be part of one, then numbered.
pub fn free_name(name: &str, machine: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut cleaned: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect();
    cleaned = cleaned.trim_matches('-').chars().take(NAME_MOST).collect();
    if name_of(&cleaned).is_err() {
        cleaned = "server".into();
    }
    let name = cleaned.as_str();
    if !taken(name) {
        return name.to_string();
    }
    let usable: String = machine
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let fit = |stem: &str, tail: &str| {
        let room = NAME_MOST.saturating_sub(tail.len() + 1);
        format!("{}-{tail}", &stem[..stem.len().min(room)])
    };
    if !usable.is_empty() {
        let named = fit(name, &usable[..usable.len().min(NAME_MOST / 2)]);
        if !taken(&named) {
            return named;
        }
    }
    (2..).map(|n| fit(name, &n.to_string())).find(|named| !taken(named)).expect("some number is free")
}

/// The servers an `mcpServers` JSON object names, as Claude Desktop, Claude
/// Code and Cursor write it; the object itself, or a document holding it.
pub fn parse_servers(text: &str) -> Result<Vec<(String, Server)>, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| format!("not JSON: {error}"))?;
    let servers = match value.get("mcpServers").or_else(|| value.get("servers")) {
        Some(Value::Object(servers)) => servers.clone(),
        Some(_) => return Err("mcpServers is not an object".into()),
        None => match value {
            Value::Object(servers) => servers,
            _ => return Err("no mcpServers object".into()),
        },
    };
    let mut found = Vec::new();
    for (name, entry) in servers {
        name_of(&name)?;
        found.push((name.clone(), server(&name, &entry)?));
    }
    Ok(found)
}

fn server(name: &str, entry: &Value) -> Result<Server, String> {
    let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
    let strings = |key: &str| -> BTreeMap<String, String> {
        entry
            .get(key)
            .and_then(Value::as_object)
            .map(|map| map.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect())
            .unwrap_or_default()
    };
    if let Some(command) = text("command") {
        let args = entry
            .get("args")
            .and_then(Value::as_array)
            .map(|args| args.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        return Ok(Server::Command(CommandMcp { command, args, env: strings("env"), cwd: text("cwd") }));
    }
    if let Some(url) = text("url").or_else(|| text("serverUrl")) {
        let sse = text("type").or_else(|| text("transport")).is_some_and(|kind| kind.eq_ignore_ascii_case("sse"));
        let remote = RemoteMcp { url, sse, headers: strings("headers") };
        remote.validate(name)?;
        return Ok(Server::Remote(remote));
    }
    Err(format!("the MCP server {name} has neither a command nor a url"))
}
