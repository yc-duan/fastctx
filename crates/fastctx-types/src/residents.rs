//! Residents: who an agent session is in the World, and its records
//! (`world-town.md`, 居民目录).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::run::{EndedHow, ExitStatus};
use crate::{AgentName, JobId, MachineName, ResidentId, TownName};

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

/// Something the World tells a resident, waiting in its inbox until the
/// resident's host carries it to the model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Notice {
    /// A job the resident started, which went on in the background, ended.
    JobEnded {
        job: JobId,
        machine: MachineName,
        how: EndedHow,
        /// How the shell itself exited, when it did.
        status: Option<ExitStatus>,
        at: Timestamp,
        /// The log holding all of its output, on that machine.
        output: String,
    },
}

/// What a resident saw of a file through `fastctx view`: one line of its
/// `seen` record. A file shown whole is remembered by its content's hash;
/// one shown in part by its size and modification time, since hashing a
/// file of several gigabytes for a glance at it would cost more than the
/// glance (`world-view.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub path: String,
    pub at: Timestamp,
    pub sha256: Option<String>,
    pub size: u64,
    pub modified: Option<Timestamp>,
    /// The encoding it was read in, for text.
    pub encoding: Option<String>,
}

/// One thing the World saw a resident do: one line of its `actions`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub at: Timestamp,
    /// Where it happened, which may be another machine than the resident's.
    pub machine: MachineName,
    /// The hand it used: `run`, `edit`, `write`, `apply_patch`, `view`.
    pub hand: String,
    /// What it did, in a line: the command's first line, the file written.
    pub what: String,
    /// How it came out: `exit 0`, `running as j-k3x9a2`, `3 edits`.
    pub result: String,
}

/// Where a resident is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Whereabouts {
    pub resident: ResidentId,
    pub town: Option<TownName>,
}
