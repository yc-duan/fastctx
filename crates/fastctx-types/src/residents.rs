//! Residents: who an agent session is in the World, and its records
//! (`world-town.md`, 居民目录).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{AgentName, MachineName, ResidentId, TownName};

/// The program a session runs in, as the session itself reported it in the
/// MCP handshake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub agent: AgentName,
    /// The client name from the MCP handshake, such as `codex-mcp-client`.
    pub client: String,
    pub version: String,
}

/// What a session knows about itself that lets the World recognise it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hints {
    /// Set by the World when it started the session itself.
    pub resident: Option<ResidentId>,
    /// The thread a Codex call belongs to; one thread is one resident,
    /// whichever connections it uses.
    pub thread: Option<String>,
}

/// A resident's `about` record: who it is and how it came to be, written
/// once when it is born.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct About {
    pub id: ResidentId,
    pub host: Host,
    pub machine: MachineName,
    pub born_at: Timestamp,
    pub origin: Origin,
    /// The session or thread id on the host's side, when there is one.
    pub session: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Origin {
    /// The user opened the session.
    User,
    /// Another resident asked for it, with this label for the task.
    Colleague { asked_by: ResidentId, label: String },
}

/// Where a resident is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Whereabouts {
    pub resident: ResidentId,
    pub town: Option<TownName>,
}
