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
    Command {
        name: String,
        args: String,
        /// The host's skill path, when a catalog selection names a skill.
        /// Equal skill names remain distinct; no arbitrary file is accepted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// Compacts the conversation now. Between turns only.
    Compact,
    /// Withdraws a control that is still waiting for the current turn to end.
    CancelControl { id: String },
    /// The next eligible request's model and effort; what is `None` stays as
    /// it is. Codex applies this when a new turn starts.
    Model { model: Option<String>, effort: Option<String> },
    /// Names the conversation.
    Rename { name: String },
    /// A new conversation carrying this one's history through the turn
    /// `through`, or all of it; the new one is a resident of its own, on the
    /// same machine and in the same town. `side` keeps it in this one's side
    /// panel rather than in the list.
    Fork {
        through: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        side: bool,
    },
    /// Drops the turn `from` and every turn after it from the conversation.
    /// Files the turns changed stay as they are.
    Rewind { from: String },
    /// The host's native agents, separate from World residents.
    Agents,
    /// The recent public messages of a native agent, identified by its host id.
    AgentMessages { agent_id: String },
    /// Answers a request the session waits on the user for, as its host
    /// reported it (`SessionChange::Request`): `request` is the request's
    /// opaque reply address, `answer` what the host's contract takes.
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
    Agents,
    AgentMessages,
}

/// What came of a `Control`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Controlled {
    /// A saved request override, without changing the user's host defaults.
    Selected {
        model: Option<String>,
        effort: Option<String>,
    },
    /// A context prefix was retained. These limitations are part of the result.
    Rewound {
        from: Option<String>,
        retained_messages: u64,
        files_restored: bool,
        compacted_turns_reachable: bool,
    },
    Agents {
        agents: Vec<super::HostAgent>,
    },
    AgentMessages {
        agent_id: String,
        messages: Vec<Value>,
        recent_only: bool,
    },
    /// Claude's tool-less completion over its own last request prefix.
    SideAnswer {
        reply: Value,
    },
    /// A confirmed action; `said` is what the host answered in words, when
    /// it did. Merely finishing an invocation is `Reported`, a veto `Refused`.
    Done {
        said: Option<String>,
    },
    /// The host completed the invocation but supplied no success/failure
    /// contract. Its words must be shown without a success assertion.
    Reported {
        said: Option<String>,
        /// An accepted deferred control. Progress is in metadata.fastctx_control.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queued: Option<String>,
    },
    /// A structured host refusal, with the host's words unchanged.
    Refused {
        said: String,
        /// Some host writers change the live selection before failing to
        /// persist it. A refusal must not claim those effects rolled back.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current_model: Option<String>,
    },
    /// Open the model selector with this catalog and the next World turn's
    /// selections. This is a UI request, not a command's textual output.
    ModelMenu {
        models: Vec<ModelOffer>,
        model: Option<String>,
        effort: Option<String>,
        service_tier: Option<String>,
    },
    /// The fork, a resident of its own.
    Forked {
        resident: Address,
    },
    /// The new resident exists, but retaining the requested prefix failed.
    ForkIncomplete {
        resident: Address,
        why: String,
    },
}

/// One accepted control and its durable progress. Terminal states also appear in history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingControl {
    pub id: String,
    pub control: Control,
    pub at: Timestamp,
    pub state: ControlState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Box<Controlled>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlState {
    Waiting,
    Running,
    Completed,
    Failed,
    Cancelled,
    Uncertain,
}

/// What a conversation offers now, asked before its menus are shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offers {
    /// This node accepts CancelControl and reports pending_controls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub control_queue: bool,
    /// This node tells each control in its conversation as it begins and as
    /// it ends, there for every device (`fastctx_control` metadata).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub controls_told: bool,
    /// Remote controls can return a nested result without colliding with the
    /// transport's reply tag (notably a side answer's own `reply` field).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub control_reply_envelope: bool,
    /// Controls accepted for later execution, including one currently executing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_controls: Vec<PendingControl>,
    /// Exact turn boundaries observed by hosts that need a message checkpoint.
    /// None means this host uses its own turn addressing rather than checkpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoints: Option<Vec<TurnCheckpoint>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_limits: Option<HistoryLimits>,
    /// The controls this session takes.
    pub controls: Vec<ControlKind>,
    /// Its own commands, as its host lists them now.
    pub commands: Vec<CommandOffer>,
    /// The models it can be given, as its host lists them; empty when the
    /// host does not list them.
    pub models: Vec<ModelOffer>,
    /// The model and effort selected for the next turn through the World,
    /// or the latest host-reported defaults when no choice was made.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The host's automatic route while the saved model override is suspended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automatic_model: Option<String>,
    /// Selected service tier for the next World turn, or the host's report
    /// without a World choice. Catalog defaults do not mean a tier is active.
    #[serde(default)]
    pub service_tier: Option<String>,
    /// How much of its context, and of the account's limits, is used, when
    /// the host says.
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCheckpoint {
    pub turn: String,
    pub rewind: bool,
    pub fork: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryLimits {
    pub files_restored: bool,
    pub compacted_turns_reachable: bool,
    pub empty_prefix_allowed: bool,
    pub complete_prefix: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandOffer {
    /// Without the slash.
    pub name: String,
    pub description: String,
    /// What it takes after its name, as the host hints it.
    pub args: Option<String>,
    /// Background argument forms, separate from the host's unrestricted hint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argument_forms: Vec<String>,
    /// Whether arguments are accepted or required. Older catalogs without
    /// this field make no requirement; keep their arguments optional.
    #[serde(default)]
    pub arguments: CommandArguments,
    /// Where it comes from: the host itself, a skill, a plugin, an MCP
    /// server, a prompt file; as the host says, kept verbatim.
    pub source: Option<String>,
    /// Pass this path unchanged with the command when selecting a skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandArguments {
    None,
    #[default]
    Optional,
    Required,
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
    #[serde(default)]
    pub service_tiers: Vec<ServiceTierOffer>,
    #[serde(default)]
    pub default_service_tier: Option<String>,
    /// The model's context window in tokens, when the World knows it: a
    /// conversation already longer than this does not fit on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceTierOffer {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the context and the window's size, when known.
    pub context: Option<ContextUsage>,
    /// The account's limits, as the host's latest answer had them.
    pub limits: Vec<Limit>,
    /// When those limits were read or last pushed; older nodes do not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits_at: Option<Timestamp>,
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
