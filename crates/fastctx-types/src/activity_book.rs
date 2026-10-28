//! The activity book (`world-layers.md`, 活动账本): what agents did with the
//! World's `run` on a machine, one deed for each script that ran there.
//! Each machine keeps its own; an interface asks each and puts them
//! together.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{AgentName, JobId, MachineName, TownName};

/// What an interface asks a machine of its book.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "ask")]
pub enum BookQuery {
    /// The deeds begun from `from` until `to`, newest first, at most
    /// `limit` of them.
    Deeds { from: Timestamp, to: Timestamp, limit: u32 },
    /// The counts of each hour from `from` until `to`, a year back at most.
    Hours { from: Timestamp, to: Timestamp },
}

/// A machine's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BookAnswer {
    /// `more` when deeds beyond the limit were left out.
    Deeds {
        machine: MachineName,
        deeds: Vec<Deed>,
        more: bool,
    },
    Hours {
        machine: MachineName,
        hours: Vec<Hour>,
    },
}

/// One script `run` ran on this machine, for a session here or, through
/// `on`, for one on `from`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deed {
    pub job: JobId,
    pub began: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Timestamp>,
    /// The machine the calling session lives on, when it is not this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<MachineName>,
    /// The calling session, as `resident@machine`; none for a page's job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Its agent, when the session lives here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentName>,
    /// Whether the session is one of Fastctx's conversations rather than
    /// one the user opened on their own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<TownName>,
    /// The script, its beginning where it is long.
    pub script: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    /// Bytes of output.
    #[serde(default)]
    pub output: u64,
    /// The other machines it went to with `on`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub went: Vec<MachineName>,
    /// Files looked at with `fastctx view`.
    #[serde(default)]
    pub looked: u32,
    /// Files copied with `fastctx copy`.
    #[serde(default)]
    pub copied: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched: Vec<Touched>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Running,
    /// Ended with exit status 0.
    Done,
    /// Ended otherwise, or could not start.
    Failed,
    /// Stopped by `fastctx stop`, the user, or a deletion.
    Stopped,
    /// Its machine lost track of it, as when the node ended under it.
    Lost,
}

/// A file a deed changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Touched {
    pub path: String,
    pub how: TouchHow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
    /// Seen changed in its repository while the script ran, rather than
    /// written by one of the World's own commands; its line counts are
    /// against the repository's last commit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub seen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchHow {
    Added,
    Changed,
    Removed,
    /// Copied here with `fastctx copy`; `path` is where the copy went.
    Copied,
}

/// What one session did in one hour.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hour {
    /// The hour's start.
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentName>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
    pub runs: u32,
    pub failed: u32,
    /// Milliseconds the deeds ran, summed.
    pub busy: u64,
    pub files: u32,
}
