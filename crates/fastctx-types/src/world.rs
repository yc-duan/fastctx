//! The World's machines as `fastctx nodes` shows them.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::MachineName;

/// What a machine is like, which a resident needs to know before acting on
/// it (`world-run.md`, 路径与各个系统的不同).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineFacts {
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
}

/// How this machine's link to the hub stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LinkState {
    /// The machine has joined no World with others: it is a World of one.
    Alone,
    Connecting {
        since: Timestamp,
        last_error: Option<String>,
    },
    /// Connected at the address `hub`: the hub's, or the relay `relay`'s
    /// when it connects through one; the connection leaves this machine by
    /// `route`.
    Online {
        since: Timestamp,
        hub: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        relay: Option<MachineName>,
        route: Route,
    },
    /// The hub refused this machine for good.
    Stopped {
        reason: String,
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

/// One upstream a machine may connect to (`world-network.md`, 一个中枢，一棵树).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Upstream {
    Hub,
    Relay { machine: MachineName },
}

/// Where a machine relays for the machines below it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelaySetting {
    /// Where it listens.
    pub listen: std::net::SocketAddr,
    /// Where the machines below it dial it, written into its record.
    pub public: Vec<std::net::SocketAddr>,
}

/// Where a member hangs in the World's tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// The upstreams it connects through, in the order it tries them; the
    /// hub is the last of any.
    pub via: Vec<Upstream>,
    /// Where machines below it dial it, when it relays.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relays_at: Vec<String>,
}

/// `fastctx nodes`: this machine's link, and every machine of the World.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodesReply {
    pub machine: MachineName,
    pub world: Option<String>,
    pub link: LinkState,
    pub machines: Vec<Node>,
    /// Members that joined with this machine's invitations and the user has
    /// not been shown yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joined: Vec<Joined>,
}

/// A member that came in with this machine's invitation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Joined {
    pub name: MachineName,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub device: bool,
    pub noticed_at: Timestamp,
}

/// Where a member stands in the World: what it may do, as every machine
/// works it out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    /// Its scope, projects by their short names; a project this machine
    /// cannot name is shown by its id.
    pub scope: crate::scope::Scope<String>,
    /// Whether it holds management rights.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub manage: bool,
    /// Who invited it; `None` for the machine that founded the World.
    pub invited_by: Option<MachineName>,
    /// Whether it is the branch of an inviter without management rights.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delegated: bool,
    /// Whether it has published its sealing key: without one, nothing can
    /// be sealed to it yet.
    #[serde(default)]
    pub sealing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub name: MachineName,
    /// `None` when whether it is connected is not known.
    pub online: Option<bool>,
    /// When it connected, if online; when it was last seen, if not.
    pub since: Option<Timestamp>,
    /// What the machine said of itself just now, or why it could not be
    /// asked.
    pub facts: Result<MachineFacts, String>,
    /// Set when the machine's key is revoked: no machine trusts it.
    pub revoked: Option<Revoked>,
    /// A device, such as a phone: a member nothing runs on, which the
    /// World does not ask or work on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub device: bool,
    /// What it may do; `None` when it is revoked, or this machine is in no
    /// World with others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing: Option<Standing>,
    /// Where it hangs in the World's tree; `None` as for `standing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<Place>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revoked {
    pub by: MachineName,
    pub at: Timestamp,
    pub reason: String,
}
