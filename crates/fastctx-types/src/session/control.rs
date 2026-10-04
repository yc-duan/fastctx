//! What the user can do to a conversation besides speaking into it, and
//! what a conversation offers for that (`world-gui.md`, 对话). Each host does
//! what its official entrances let it: Codex through its app-server, Claude
//! through the World's mod and the `claude` command. What a session cannot
//! do is not offered, and when it is asked anyway the answer says why; it
//! is never imitated.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::Address;

/// One thing done to a conversation, for the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum Control {
    /// Runs one of the session's own commands as the user typing
    /// `/name args` would; `name` is without the slash.
    Command { name: String, args: String },
    /// Compacts the conversation now. Between turns only.
    Compact,
    /// The model and reasoning effort from the next turn on; what is `None`
    /// stays as it is.
    Model { model: Option<String>, effort: Option<String> },
    /// Names the conversation.
    Rename { name: String },
    /// A new conversation carrying this one's history through the turn
    /// `through`, or all of it; the new one is a resident of its own, on the
    /// same machine and in the same town.
    Fork { through: Option<String> },
    /// Drops the turn `from` and every turn after it from the conversation.
    /// Files the turns changed stay as they are.
    Rewind { from: String },
    /// Answers a request the session waits on the user for, as its host
    /// reported it (`SessionChange::Request`): `request` is the request's
    /// id, `answer` what the host's contract for that request takes.
    Answer { request: Value, answer: Value },
}

/// The kinds of `Control`, for saying which a conversation takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Command,
    Compact,
    Model,
    Rename,
    Fork,
    Rewind,
    Answer,
}

/// What came of a `Control`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Controlled {
    /// Done; `said` is what the host answered in words, when it did (a
    /// command's output, a compaction that was skipped and why).
    Done { said: Option<String> },
    /// The fork, a resident of its own.
    Forked { resident: Address },
}

/// What a conversation offers now, asked before its menus are shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offers {
    /// The controls this session takes.
    pub controls: Vec<ControlKind>,
    /// Its own commands, as its host lists them now.
    pub commands: Vec<CommandOffer>,
    /// The models it can be given, as its host lists them; empty when the
    /// host does not list them.
    pub models: Vec<ModelOffer>,
    /// The model and effort it uses now, when known.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// How much of its context, and of the account's limits, is used, when
    /// the host says.
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOffer {
    /// Without the slash.
    pub name: String,
    pub description: String,
    /// What it takes after its name, as the host hints it.
    pub args: Option<String>,
    /// Where it comes from: the host itself, a skill, a plugin, an MCP
    /// server, a prompt file; as the host says, kept verbatim.
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelOffer {
    pub id: String,
    /// The name to show, when the host gives one apart from the id.
    pub name: Option<String>,
    pub description: Option<String>,
    /// The reasoning efforts it takes, in the host's own words.
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    /// Whether the host uses it when none is chosen.
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the context and the window's size, when known.
    pub context: Option<ContextUsage>,
    /// The account's limits, as the host's latest answer had them.
    pub limits: Vec<Limit>,
    /// What the session has cost so far, in US dollars, when the host says;
    /// kept as the host's number.
    pub cost_usd: Option<Number>,
    /// The host's own report, for what the fields above do not carry.
    pub raw: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub used: u64,
    pub window: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limit {
    /// The host's name for the window, such as `5h` or `weekly`.
    pub name: String,
    /// Kept as the host's number.
    pub used_percent: Option<Number>,
    pub resets_at: Option<Timestamp>,
}
