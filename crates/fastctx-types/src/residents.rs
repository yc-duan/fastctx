//! Residents: who an agent session is in the World, and its records
//! (`world-town.md`, 居民目录).

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::run::{EndedHow, ExitStatus};
use crate::session::{Health, Speaker, TurnEnd};
use crate::{Address, AgentName, JobId, MachineName, NoticeId, ResidentId, TownName};

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
    /// The resident this session arrived as before, when its connection to
    /// the node broke and it arrives again, as after the node restarted.
    #[serde(default)]
    pub was: Option<ResidentId>,
    /// The Claude tool call's `_meta["claudecode/toolUseId"]`. Its mod
    /// publishes the same id before allowing this particular call through.
    #[serde(default)]
    pub tool_use_id: Option<String>,
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
    /// The World started the session and reaches it itself: a thread of
    /// this machine's Codex app-server, or a Claude Code background session.
    /// Any other session is the user's own, which the World only hears
    /// through its calls of the World's tools.
    #[serde(default)]
    pub world_started: bool,
    /// The model, as the host told the World; `None` when it did not.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Origin {
    /// The user opened the session.
    User,
    /// Another resident asked for it, with this label for the task.
    Colleague { asked_by: Address, label: String },
}

/// Something the World tells a resident: handed to its host when the host
/// gives the World a way in, otherwise waiting in its inbox until its next
/// call of a World tool. The id names it on the resident's machine and in
/// the letter that carries it from another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: NoticeId,
    pub at: Timestamp,
    pub notice: Notice,
}

/// The last lines of a job's output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastLines {
    pub text: String,
    /// Whether the text is all of the output.
    pub whole: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Notice {
    /// A job the resident started, which went on in the background, ended.
    JobEnded {
        job: JobId,
        machine: MachineName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<NoticeId>,
        /// Where this hop's CLI took the end, for arbitration of nested calls.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follower: Option<MachineName>,
        how: EndedHow,
        /// How the shell itself exited, when it did.
        status: Option<ExitStatus>,
        at: Timestamp,
        /// The log holding all of its output, on that machine.
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr: Option<String>,
        /// The end of that output as a terminal shows it, so the resident
        /// knows how it went without reading the log.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last: Option<LastLines>,
    },
    /// A turn of a colleague ended, which the resident's words went into or
    /// which it asked for: its last message, verbatim.
    Answer { colleague: Address, label: String, turn: String, end: TurnEnd },
    /// A colleague's host reported something in a turn owed to the resident
    /// that it must hear at once.
    Health { colleague: Address, label: String, health: Health },
    /// Words someone said to the resident.
    Words { from: Speaker, text: String },
    /// The user's words and files; the accompanying facts are the World's.
    Attached { message: crate::session::attachments::AttachedMessage, text: String },
}

/// How words for a resident landed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Landed {
    /// Submission crossed a host boundary without an acceptance result.
    /// Its exact words remain recorded, and are not submitted again.
    Uncertain { why: String },
    /// It was idle: a new turn started with them.
    NewTurn { turn: String },
    /// It was working: they join its running turn at its next tool boundary.
    JoinedTurn { turn: String },
    /// Its host gives the World no way in now: they wait for its next call
    /// of a World tool, or for it to come back. `why` is set when the World
    /// has a way in and it failed. `answered` says whether the end of the
    /// turn they go into will be told: the World sees the turns of the
    /// sessions it holds and of Claude sessions with its plugin, no others.
    Waiting {
        why: Option<String>,
        #[serde(default)]
        answered: bool,
    },
    /// It lives on another machine: they went there as a letter, which that
    /// machine hands over when it has it.
    Sent,
    /// Handed to the World's plugin in its session, which brings them in at
    /// once when it is idle and at its next tool boundary when it is busy.
    Handed { id: NoticeId },
}

/// What a resident is doing now, and what that commits the World to: its
/// `now`, rewritten whenever it changes. Kept for residents whose host
/// tells the World when they work: Codex threads the World holds, and
/// Claude sessions through the World's mod.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Now {
    pub activity: Activity,
    pub since: Timestamp,
    /// The turn running now, as its host named it.
    pub turn: Option<String>,
    /// For each turn running, whose words went into it: each is owed its
    /// end. Once a turn ends, its answers wait in the `outbox` until each
    /// reader's machine has its own.
    #[serde(default)]
    pub owed: BTreeMap<String, Vec<Address>>,
    /// Who the latest completed turn was for. A turn the World
    /// starts with a fact of the resident's own work, such as a job's end,
    /// is owed to them: they are who it is working for.
    pub answering: Vec<Address>,
    /// The latest thing that went wrong: what its host reported about the
    /// session, or how its last turn ended when it did not complete.
    pub trouble: Option<Trouble>,
    /// It sits at this machine's Computer Use desk until its turn ends.
    #[serde(default)]
    pub desk: bool,
    /// When the World first confirmed this turn holds the desk.
    #[serde(default)]
    pub desk_since: Option<Timestamp>,
    /// The running turn cannot continue until a user answers its host.
    #[serde(default)]
    pub waiting_for_user: bool,
    #[serde(default)]
    pub waiting_since: Option<Timestamp>,
    /// The conversation's name, as its host last gave it.
    #[serde(default)]
    pub title: Option<String>,
    /// Latest tool start observed from the host, even when it is not a World tool.
    #[serde(default)]
    pub last_tool_at: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    Busy,
    Idle,
    /// Not here: a colleague whose requester left, or a session that ended.
    /// Words wake it.
    Asleep,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trouble {
    pub at: Timestamp,
    pub text: String,
}

/// One line of a colleague's `messages`: what it was asked, what it was
/// told, how each turn ended and how its answers reached their readers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageLine {
    pub at: Timestamp,
    #[serde(flatten)]
    pub what: MessageKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MessageKind {
    /// The request it was born with.
    Asked { from: Address, text: String },
    /// Words that went into one of its turns.
    Heard { from: Speaker, turn: Option<String>, text: String },
    /// A turn ended, owed to `owed`.
    Ended { turn: String, end: TurnEnd, owed: Vec<Address> },
    /// How word of a turn reached one who was owed it.
    Told { to: Address, turn: Option<String>, outcome: Outcome },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Outcome {
    /// Handed to its host now.
    Delivered,
    /// Waiting on its machine until it can be handed over; `why` is set
    /// when handing it over now failed.
    Waiting {
        why: Option<String>,
    },
    /// Sent as a letter to its machine.
    Sent,
    Failed {
        reason: String,
    },
}

/// What a resident saw through view. Text decoding validates and hashes the
/// complete byte stream even when only selected lines are displayed. Other
/// partial views can record size and modification time without a full hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub path: String,
    pub at: Timestamp,
    pub sha256: Option<String>,
    pub size: u64,
    pub modified: Option<Timestamp>,
    /// The encoding it was read in, for text.
    pub encoding: Option<String>,
    /// The user's explicit codec or per-line codec order; inferred labels are not overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_selection: Option<String>,
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

/// Left in the directory a resident had in a town when it moved to another
/// (`world-town.md`): what it did there stays there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moved {
    /// The town it went to; `None` for outside every town.
    pub to: Option<TownName>,
    pub at: Timestamp,
}

/// Something on its way into a resident's session: one file in its
/// `inbox`, kept until the World knows which turn took it, so whoever is
/// owed that turn's end is owed it from then on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub delivery: Delivery,
    /// Who is owed the end of the turn it goes into.
    pub readers: Vec<Address>,
    pub state: PendingState,
    /// Resolve the audience when these words actually enter a turn, after
    /// any earlier submission has recorded its readers.
    #[serde(default)]
    pub working: bool,
    /// A Claude conversation already entrusted with these words. A later
    /// /clear must not move them into the replacement conversation.
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingState {
    /// It waits for a way in: the next words that go into the session, the
    /// World's plugin in it, or its next call of a World tool.
    Waiting,
    /// It is being put into a Codex thread; the app-server's answer says
    /// which turn took it.
    Sending,
    /// The World's plugin in a Claude session has it, and says which turn
    /// takes it.
    Handed,
    /// A Claude session stopped by its supervisor was started again with it
    /// as its first words: the session's next turn takes it.
    Waking,
}

/// A turn's end on its way to one reader: kept from the moment the turn
/// ends until the reader's machine has it. One file in the `outbox` of the
/// resident whose turn it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owing {
    pub to: Address,
    pub delivery: Delivery,
}

/// Where a resident is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Whereabouts {
    pub resident: ResidentId,
    pub town: Option<TownName>,
}
