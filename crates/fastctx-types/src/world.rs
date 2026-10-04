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
    /// Connected to the hub at `hub`; `route` says the way the connection
    /// leaves this machine.
    Online {
        since: Timestamp,
        hub: String,
        route: String,
    },
    /// The hub refused this machine for good.
    Stopped {
        reason: String,
    },
}

/// `fastctx nodes`: this machine's link, and every machine of the World.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodesReply {
    pub machine: MachineName,
    pub world: Option<String>,
    pub link: LinkState,
    pub machines: Vec<Node>,
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revoked {
    pub by: MachineName,
    pub at: Timestamp,
    pub reason: String,
}
