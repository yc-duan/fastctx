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
    /// What the World keeps here, by kind and by conversation. With `kept`,
    /// answered at once with what was last measured here (`Unmeasured`
    /// while nothing was), so a page shows it while a measure goes on:
    /// measuring walks everything kept here. An earlier node, not knowing
    /// it, measures.
    List {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        kept: bool,
    },
    /// Deletes conversations kept here, each by the name `List` gives it: a
    /// conversation that lives here with everything it has here, its host's
    /// own record included; a guest room alone.
    Delete { names: Vec<String> },
    /// How many days full command output stays after the model got its
    /// answer; sets it first when `days` is given.
    OutputDays { days: Option<u32> },
    /// Empties this machine's viewing cache, coordinating with its viewers.
    ClearCache,
    /// Whether each account logged in here goes on taking in its copies of
    /// the conversations living on other machines, and how many of those it
    /// holds here taken in no more (`fastctx data copies`).
    Copies,
    /// Sets whether `account` of the hub `hub` takes in its copies here, as
    /// the machine's owner would: taken in again, what passed it by comes
    /// down; paused, what is here stays, read here no more.
    SetCopies { hub: String, account: String, on: bool },
    /// Lets go, as the machine's owner would, the copies here `account` of
    /// the hub `hub` reads that may be let go — every reader of each here
    /// paused, none living here, nothing of it written here — or, without
    /// `apply`, counts them. Their room comes back once the library is
    /// compacted (`fastctx data compact`).
    ClearCopies {
        hub: String,
        account: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        apply: bool,
        /// Only these, where given: those a count said (`Cleared::keys`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only: Option<Vec<String>>,
    },
    /// The application was opened here as `account` of the hub `hub`,
    /// opened at `opened` (milliseconds): it takes in its copies here from
    /// now on, unless the machine's owner paused them. Sent by the
    /// application alone, never by a command.
    AppOpened { hub: String, account: String, opened: i64 },
    /// The conversations' scopes this machine began that their hub will
    /// never describe — begun for an account gone from it, or for an earlier
    /// account of its name — each with its conversation (`fastctx data
    /// undescribed`): what of a removed account is left here, to look at and
    /// delete.
    Undescribed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "data")]
pub enum DataReply {
    Listed(Box<Space>),
    /// Nothing measured here yet since this node started (`List` with `kept`).
    Unmeasured,
    Deleted {
        deletions: Vec<Deletion>,
    },
    OutputDays {
        days: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_retention: Option<String>,
    },
    CacheCleared {
        bytes: u64,
    },
    Copies {
        accounts: Vec<CopiesOf>,
    },
    Undescribed {
        scopes: Vec<Undescribed>,
    },
    Cleared(Cleared),
}

/// What a machine's owner asks from afar of the copies the machine holds of
/// conversations living elsewhere (`RemoteCall::Copies`, the machine page's
/// 会话副本): as `DataCall::Copies`, `SetCopies` and `ClearCopies` ask of
/// this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum CopiesAsk {
    Report,
    Set {
        hub: String,
        account: String,
        on: bool,
    },
    Clear {
        hub: String,
        account: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        apply: bool,
        /// Only these, where given: those a count said (`Cleared::keys`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only: Option<Vec<String>>,
    },
}

/// What a machine answers its owner of its copies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "copies")]
pub enum CopiesAnswer {
    Report { accounts: Vec<CopiesOf> },
    Cleared(Cleared),
}

/// What letting go an account's copies here came to, or would
/// (`DataCall::ClearCopies`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cleared {
    /// They were let go; only counted otherwise.
    pub applied: bool,
    /// How many copies were let go, or would be.
    pub scopes: u64,
    /// About how many bytes they took.
    pub bytes: u64,
    /// Those kept, each with why.
    #[serde(default)]
    pub kept: Vec<(String, String)>,
    /// The scopes let go, or that would be: what letting go after a count
    /// is limited to.
    #[serde(default)]
    pub keys: Vec<String>,
    /// Why letting go stopped short after some were let go: those are
    /// said above, the rest left as they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

/// A scope its hub will never describe (`DataCall::Undescribed`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Undescribed {
    pub scope: String,
    /// The conversation it is of, by the name `List` and `Delete` know it by.
    pub conversation: String,
    /// The conversation is still kept here.
    pub kept: bool,
    pub why: String,
    pub at: Timestamp,
}

/// Whether an account logged in here takes in its copies of conversations
/// living elsewhere (`DataCall::Copies`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiesOf {
    pub hub: String,
    pub account: String,
    /// When the account was opened, in milliseconds.
    pub opened: i64,
    pub on: bool,
    /// What set it so: `install` (as this machine was found installed),
    /// `app` (the application was opened here), `owner`; none while it is
    /// not known yet whether the application is installed here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
    /// The conversations' scopes it reads held here and taken in no more.
    pub paused_scopes: u64,
    /// About how many bytes those take here.
    #[serde(default)]
    pub paused_bytes: u64,
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
    /// An explicit policy overrides the legacy days field when recognized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_retention: Option<String>,
    /// How long each Claude Code login here keeps its own session records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claude_logins: Vec<ClaudeKeeping>,
    /// Entries under the root that could not be read and are not counted
    /// (a file another program holds, a directory refused), and the first
    /// of them with why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<Unread>,
}

/// What measuring skipped (`Space::unread`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unread {
    pub entries: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<String>,
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
    /// It is deleted: `bytes` go in all. When `finishing` says why, some of
    /// it is still being cleared, which the node goes on with by itself;
    /// nothing of it is listed or takes anything new meanwhile.
    Deleted {
        bytes: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finishing: Option<String>,
    },
    /// Nothing was deleted, for this reason.
    Refused { why: String },
    /// Deleting began and stopped before anything that cannot be undone;
    /// what is left is still listed and can be deleted again.
    Failed { why: String },
}
