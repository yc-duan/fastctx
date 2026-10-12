//! What the World keeps on one machine, and deleting it (`fastctx data`, the
//! settings page 本机数据). Each machine manages only its own: these requests
//! are answered by the node of the machine they are made on, and never go to
//! another.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{AgentName, MachineName, TownName};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum DataCall {
    /// What the World keeps here, by kind and by conversation.
    List,
    /// Deletes conversations kept here, each by the name `List` gives it: a
    /// conversation that lives here with everything it has here, its host's
    /// own record included; a guest room alone.
    Delete { names: Vec<String> },
    /// How many days full command output stays after the model got its
    /// answer; sets it first when `days` is given.
    OutputDays { days: Option<u32> },
    /// Empties this machine's viewing cache, coordinating with its viewers.
    ClearCache,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "data")]
pub enum DataReply {
    Listed(Box<Space>),
    Deleted { deletions: Vec<Deletion> },
    OutputDays { days: u32 },
    CacheCleared { bytes: u64 },
}

/// The days full command output may be kept for: what the user chooses from.
pub const OUTPUT_DAYS: [u32; 5] = [1, 3, 7, 14, 30];

/// What the World keeps on this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub machine: MachineName,
    /// When it was measured.
    pub at: Timestamp,
    /// The directory fastctx keeps everything in here, and how much is in it:
    /// what `du` counts there.
    pub root: String,
    pub root_bytes: u64,
    pub kinds: Kinds,
    pub conversations: Vec<ConversationSpace>,
    /// How many days full command output stays after the model got its
    /// answer.
    pub output_days: u32,
    /// How long each Claude Code login here keeps its own session records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claude_logins: Vec<ClaudeKeeping>,
}

/// What the World keeps here, by kind, in bytes. All but `host_records` is in
/// the root directory, and those add up to `root_bytes`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kinds {
    /// The conversations' own records and the originals of what the user
    /// attached.
    pub conversations: u64,
    /// The record the agent's own program keeps of each conversation listed,
    /// outside the root directory.
    pub host_records: u64,
    /// The commands they ran: records, output and temporary files.
    pub command_output: u64,
    /// What the World keeps only to be quicker, made again when gone.
    pub cache: u64,
    /// fastctx's own programs.
    pub program: u64,
    /// Everything else: projects, the link to the network, logs, this
    /// machine's identity.
    pub other: u64,
}

/// Which list a conversation is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    /// It lives here and is in the sidebar.
    Listed,
    /// It lives here; the user removed it, and its data is still here.
    Removed,
    /// It lives on another machine and worked here: its guest room.
    Elsewhere,
}

/// One conversation's directory here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSpace {
    /// Its directory's name, which `Delete` takes: its id, or `id@machine`
    /// for a guest room.
    pub name: String,
    pub group: Group,
    pub agent: AgentName,
    /// What the conversation is called; none when it has no name yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The project it is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub town: Option<TownName>,
    /// The machine it lives on; none for copies whose machine was not
    /// recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lives_on: Option<MachineName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<Timestamp>,
    /// Its parts added up.
    pub bytes: u64,
    pub parts: Parts,
    /// Files attached for a conversation that was never opened.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unopened: bool,
    /// Why it cannot be deleted now, when it cannot: what to do first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
}

/// One conversation's bytes here, by part.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parts {
    /// Its records and the originals of what the user attached.
    pub records: u64,
    /// The commands it ran here.
    pub command_output: u64,
    /// The record its agent's own program keeps of it.
    pub host: HostRecord,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kept")]
pub enum HostRecord {
    /// There is none here: a guest room, or a conversation never opened.
    #[default]
    None,
    Kept {
        bytes: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        days: Option<u32>,
    },
    /// Claude Code deleted it itself, as it does with sessions untouched for
    /// `days` days; `days` is none when its setting could not be read.
    GoneByClaude {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        days: Option<u32>,
        #[serde(default, skip_serializing_if = "is_zero")]
        bytes: u64,
    },
    /// It is not where the agent's program keeps such records.
    Missing {
        #[serde(default, skip_serializing_if = "is_zero")]
        bytes: u64,
    },
}

impl HostRecord {
    pub fn bytes(&self) -> u64 {
        match self {
            Self::Kept { bytes, .. } | Self::GoneByClaude { bytes, .. } | Self::Missing { bytes } => *bytes,
            _ => 0,
        }
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// How long one Claude Code login keeps the records of its sessions: those
/// untouched for `days` days it deletes itself (its `cleanupPeriodDays`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeKeeping {
    /// The login's directory; none for the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// The days it keeps them, as its settings say; none when they could not
    /// be read, with why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
    /// Whether its settings name the days, rather than Claude Code's default.
    pub set: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreadable: Option<String>,
}

/// What became of one conversation `Delete` named.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deletion {
    pub name: String,
    pub outcome: DeletionOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum DeletionOutcome {
    /// Everything of it here is gone: `bytes` freed in all.
    Deleted { bytes: u64 },
    /// Nothing was deleted, for this reason.
    Refused { why: String },
    /// Deleting began and stopped here; what is left is still listed.
    Failed { why: String },
}
