//! The World's machines as `fastctx nodes` shows them.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::MachineName;

/// What kind of device a machine is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form {
    Desktop,
    Laptop,
    Server,
}

/// A kind of device a later release adds is drawn as a computer.
impl crate::tolerant::Tolerant for Form {
    const ALL: &'static [Self] = &[Form::Desktop, Form::Laptop, Form::Server];
    const UNKNOWN: Self = Form::Desktop;
}

/// What a machine is like, which a resident needs to know before acting on
/// it (`world-run.md`, 路径与各个系统的不同).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineFacts {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal_runtime: bool,
    /// Opens a colleague's goal before its first input, without an ordinary initial-turn reply.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal_colleagues: bool,
    /// Reports autonomous colleague goals and their outstanding completion promises.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal_waits: bool,
    /// Receives colleague questions without waking an idle conversation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal_questions: bool,
    /// Recognizes nonblocking question metadata on ordinary resident words.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal_question_words: bool,
    /// Routes resident goal commands to their home and supports requester controls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal_commands: bool,
    /// Receives immutable colleague goal reports and wakes closed requesters.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal_reports: bool,
    /// Reports a requester's pending inputs, running turns and undelivered answers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub turn_debts: bool,
    pub name: MachineName,
    /// `windows`, `linux` or `macos`.
    pub os: String,
    pub arch: String,
    /// Paths as the machine writes them: `/…` on Unix, `C:/…` on Windows.
    pub home: String,
    pub temp: String,
    pub path_separator: char,
    /// Whether file names that differ only in case name different files,
    /// as the platform has it by default.
    pub case_sensitive: bool,
    pub version: String,
    /// Accepts confirmed end delivery separately from CLI consumption.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub completion_receipts: bool,
    /// Accepts durable completion receipts and intentional job stops as mail.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub control_letters: bool,
    /// Stops only the dispatch named by a durable StopRun control.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub run_stops: bool,
    /// Understands sentinel account settings and lifecycle calls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sentinels: bool,
    /// Preserves internal observation identity through on and its supervisor.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sentinel_look: bool,
    /// Keeps what arrived of a shared-input upload that broke, and takes it
    /// up again from there (`RemoteCall::Upload::resumes`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub upload_resumes: bool,
    /// Retains attachment originals while surviving forks reference them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub attachment_references: bool,
    /// Answers `RemoteCall::Stats`: its details page shows what it says of
    /// itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stats: bool,
    /// Account configuration and command MCP service calls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mcp: bool,
    /// Lists private import metadata and rereads selected URL definitions.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mcp_import: bool,
    /// Keeps page consent and execution scoped to one account.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub page_accounts: bool,
    /// Answers attachment destination queries before receiving any file bytes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub attachment_places: bool,
    /// Takes the attachments a script is sent with into its scratch before
    /// it runs (`RemoteCall::On::attachments`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub attachment_runs: bool,
    /// Answers an account's replica-directory query without returning bodies.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub memory_folders: bool,
    /// Reads collaboration trees, including native child-agent histories.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collaboration_tree: bool,
    /// The bash `run` uses there, such as `bash 5.2.37 (Git for Windows,
    /// MSYS)`: on MSYS, arguments shaped like Unix paths are rewritten
    /// before a Windows program sees them (`world-run.md`). Empty when the
    /// machine runs an older fastctx that does not say.
    #[serde(default)]
    pub shell: String,
    /// What kind of device it is, as an interface draws it; older nodes do
    /// not say.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::tolerant::read_option")]
    pub form: Option<Form>,
    /// The running node's own observation; older nodes do not report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_lifetime: Option<Box<crate::node_lifetime::NodeLifetime>>,
}

/// How this machine's link to the hub stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LinkState {
    /// The computer is logged in to no hub: it is a World of one.
    Alone,
    Connecting {
        since: Timestamp,
        last_error: Option<String>,
    },
    /// Connected at the address `hub`: the hub's, or, when its own line is
    /// down, the address of the machine `through` that forwards to the
    /// hub; the connection leaves this machine by `route`.
    Online {
        since: Timestamp,
        hub: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        through: Option<String>,
        route: Route,
        /// The round trip to the hub in milliseconds, as last measured.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ms: Option<u32>,
    },
    /// The hub removed this computer, or, `dissolved`, its whole network.
    Stopped {
        reason: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dissolved: bool,
    },
}

/// The way a connection to the hub leaves this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Route {
    /// The hub is on this machine.
    Local,
    /// Pinned to a physical network card, past any VPN's routes.
    Card { name: String },
    /// Through the system's proxy.
    Proxy { proxy: String },
    /// Wherever the system's routes send it.
    System,
}

impl std::fmt::Display for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Route::Local => f.write_str("on this machine"),
            Route::Card { name } => write!(f, "directly through \"{name}\""),
            Route::Proxy { proxy } => write!(f, "through {proxy}"),
            Route::System => f.write_str("along the system's routes"),
        }
    }
}

/// How this computer's calls to a machine go.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Path {
    /// Over a direct link between the two, unseen by the hub; `ms` is its
    /// round trip as last measured.
    Direct {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ms: Option<u32>,
    },
    /// Through the hub; `why`, when the machine allows direct connections,
    /// says why there is no direct link now.
    Hub {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
}

/// `fastctx nodes`: this computer's links, and every machine its accounts
/// may use, hub by hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodesReply {
    pub machine: MachineName,
    /// What this machine is like; a device has none to tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<MachineFacts>,
    pub hubs: Vec<HubNodes>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubNodes {
    pub hub: crate::access::HubId,
    pub name: crate::HubName,
    pub link: LinkState,
    /// The accounts this computer is logged in to there.
    pub logins: Vec<crate::AccountName>,
    /// The account this computer is registered under there.
    pub home: Option<crate::AccountName>,
    /// What each of the logins may do on this computer itself, as the hub's
    /// directory decides it; none from a node before 1.0.1-beta.10, and none
    /// from a phone, which is no machine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub here: Vec<(crate::AccountName, crate::access::Rights)>,
    pub machines: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub name: MachineName,
    /// How it is written where its name is on more than one hub: `name.hub`.
    pub written: String,
    /// `None` when whether it is connected is not known.
    pub online: Option<bool>,
    /// When it connected, if online; when it was last seen, if not.
    pub since: Option<Timestamp>,
    /// The machine it reaches the hub through, when its own line is down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<String>,
    /// Whether the network's administrator allowed direct connections to
    /// it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allows_direct: bool,
    /// How this computer's calls to it go now; `None` from an older node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Path>,
    /// What the machine said of itself just now, or why it could not be
    /// asked.
    pub facts: Result<MachineFacts, String>,
    /// The account it is registered under.
    pub owner: Option<crate::AccountName>,
    /// The group its owner put it in, which only says how it is shown
    /// (`world-network.md`, 机器的分组).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// What each of this computer's accounts may do on it.
    pub rights: Vec<(crate::AccountName, crate::access::Rights)>,
}

/// Machines a session may work on that their owner put in one group, as the
/// session is told of them: the group, named for the session (an owner
/// other than the accounts it works for goes before it, `bo's lab A`), and
/// the machines as written, the one its shell is on among them when it is in
/// the group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineGroup {
    pub name: String,
    pub machines: Vec<String>,
}

/// The other machines a session may work on as a model is told them
/// (`world-network.md`, 机器的分组): those in no group first, then each group
/// with its own, the machine its shell is on, `here`, left out.
pub fn listed(others: &[String], groups: &[MachineGroup], here: Option<&str>) -> String {
    let grouped: Vec<&String> = groups.iter().flat_map(|group| &group.machines).collect();
    let loose: Vec<&str> = others.iter().filter(|name| !grouped.contains(name)).map(String::as_str).collect();
    let mut parts = Vec::new();
    if !loose.is_empty() {
        parts.push(loose.join(", "));
    }
    for group in groups {
        let machines: Vec<&str> =
            group.machines.iter().map(String::as_str).filter(|name| Some(*name) != here).collect();
        if !machines.is_empty() {
            parts.push(format!("group {}: {}", group.name, machines.join(", ")));
        }
    }
    parts.join("; ")
}

/// The group the machine a session's shell is on, `here`, is in, as the
/// session is told of it.
pub fn group_of<'a>(groups: &'a [MachineGroup], here: &str) -> Option<&'a str> {
    groups.iter().find(|group| group.machines.iter().any(|name| name == here)).map(|group| group.name.as_str())
}

/// The World as an interface shows it (`world-gui.md`): this computer, who
/// is on each machine as far as the accounts here may see, the machines hub
/// by hub, the projects, and the accounts with what they see. The node
/// reads it from what it holds, asking no machine, and tells it again
/// whenever it changes (`world-network.md`, 近况).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldNow {
    pub here: MachineName,
    pub who: crate::town::WorldWho,
    pub nodes: NodesReply,
    pub towns: Vec<crate::project::TownSummary>,
    pub accounts: crate::accounts::AccountsReply,
}
