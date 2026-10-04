//! Everything a local client can ask the node, and the answers.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::copy::{CopyReport, Place};
use crate::project::{Attachment, JournalSummary, JournalView, MapView, Record, RecordFields, Stamp, TownSummary};
use crate::residents::{Delivery, Hints, Host, Landed, Seen, Whereabouts};
use crate::run::{RunReply, RunRequest};
use crate::town::{LookedUp, WorldWho};
use crate::world::NodesReply;
use crate::write::{WriteOp, WriteOutcome};
use crate::{Address, AgentName, EntryId, JobId, JournalName, MachineName, NoticeId, ResidentId, TownName};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "call")]
pub enum Call {
    /// A session says who it is; the node finds or makes its resident.
    Arrive { host: Host, hints: Hints },
    /// Run a script for a resident of this machine, on this machine or on
    /// the machine `node` names.
    Run { resident: ResidentId, request: RunRequest, node: Option<MachineName> },
    /// Writes files for a resident of this machine, on this machine or on
    /// the machine `node` names.
    Write { resident: ResidentId, op: WriteOp, node: Option<MachineName> },
    /// Copies files or a directory from one machine to another.
    Copy { from: Place, to: Place },
    /// Asks for a colleague for a resident of this machine
    /// (`ask_colleague`): a Codex resident started on this machine or on
    /// the machine `node` names, with `prompt` as its first words. `label`
    /// is the task's few words.
    Ask { resident: ResidentId, label: String, prompt: String, node: Option<MachineName> },
    /// Words from a resident of this machine to another resident, on any
    /// machine (`message_colleague`); `machine` is where it lives, when the
    /// speaker said. The speaker is owed the end of the turn they go into.
    Message { resident: ResidentId, to: ResidentId, machine: Option<MachineName>, text: String },
    /// Records what a resident saw of files on this machine through
    /// `fastctx view`; the resident may live on another machine.
    Saw { owner: Address, town: Option<TownName>, seen: Vec<Seen> },
    /// Who is in a town and what each is doing, asking each machine it spans
    /// (`fastctx w`); without a town, the caller's, or the World as a whole
    /// for a caller in none. With `all`, also the residents who have left.
    Who { all: bool, town: Option<TownName> },
    /// One resident or job in detail, by its id, from every machine that
    /// has it (`fastctx w <id>`).
    Look { ids: Vec<String> },
    /// The user's words to a resident on any machine (`fastctx talk`);
    /// `machine` is where it lives, when the user said.
    Talk { to: ResidentId, machine: Option<MachineName>, text: String },
    /// Watches a resident on any machine: the node answers with its history
    /// and then each event as it happens, for as long as the connection
    /// stays open.
    Watch { resident: ResidentId, machine: Option<MachineName> },
    /// Opens a session for the user: a resident of `agent` started on this
    /// machine or on `node`, in `town`, with the user's words as its first.
    Open { agent: AgentName, town: Option<TownName>, node: Option<MachineName>, text: String },
    /// What waits to be handed to a resident with the answer of a World
    /// tool, oldest first, with what it is to be told of where it is. It is
    /// handed to no other answer meanwhile.
    Collect { resident: ResidentId },
    /// What `Collect` gave went out with the answer: it is handed over.
    Taken { resident: ResidentId, ids: Vec<NoticeId> },
    /// `fastctx project`.
    Project(ProjectCall),
    /// `fastctx journal`.
    Journal(JournalCall),
    /// A supervisor says its job's record has changed: the shell started
    /// or exited, the output ended, or nothing of the job runs any more.
    JobChanged { job: JobId, owner: Address, town: Option<TownName> },
    /// Joins this machine to a World with a code, under `name` or the name
    /// it has.
    Join { code: String, name: Option<MachineName> },
    /// Makes an invitation for one more machine.
    Invite { name: Option<MachineName>, expires: Timestamp },
    /// Revokes a member's key, so no machine trusts it any more.
    Revoke { name: MachineName, reason: String },
    /// This machine leaves its World and becomes a World of one again.
    Leave,
    /// This machine's link and every machine of the World (`fastctx nodes`).
    Nodes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum Reply {
    Arrived(Whereabouts),
    Run(RunReply),
    Written {
        outcome: WriteOutcome,
    },
    Copied(CopyReport),
    Asked {
        colleague: Address,
    },
    Spoken {
        to: Address,
        landed: Landed,
    },
    Opened {
        resident: Address,
        landed: Landed,
    },
    /// What waits for a resident, and lines for the head of its answer.
    Notices {
        notices: Vec<Delivery>,
        notes: Vec<String>,
    },
    Who(WorldWho),
    Towns {
        towns: Vec<TownSummary>,
    },
    Record {
        town: TownName,
        record: Box<Record>,
        opened: Stamp,
    },
    Map {
        map: Box<MapView>,
    },
    Entered {
        town: TownName,
        map: Box<MapView>,
    },
    Journals {
        town: TownName,
        journals: Vec<JournalSummary>,
    },
    Journal {
        view: Box<JournalView>,
    },
    EntryAdded {
        town: TownName,
        journal: JournalName,
        id: EntryId,
        attachments: Vec<Attachment>,
    },
    Looked {
        looked: Vec<LookedUp>,
    },
    Joined {
        world: String,
        name: MachineName,
    },
    Code {
        code: String,
    },
    Nodes(NodesReply),
    Done,
}

/// `fastctx project`: the towns, their records and maps, and entering one.
/// Without a town, a call means the caller's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum ProjectCall {
    /// Opens a town with the first version of its record.
    New {
        town: TownName,
        fields: RecordFields,
    },
    /// Writes the next version of a town's record; what is not given stays.
    Edit {
        town: Option<TownName>,
        fields: RecordFields,
    },
    /// The towns this machine keeps a ledger for.
    List,
    /// A town's record as its latest version says.
    Show {
        town: Option<TownName>,
    },
    Map {
        town: Option<TownName>,
    },
    /// The calling resident lives in `town` from now on.
    Enter {
        town: TownName,
    },
}

/// `fastctx journal`: a town's journals. Without a town, a call means the
/// caller's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum JournalCall {
    /// Adds an entry, starting the journal when `description` is given and
    /// it has none.
    Add {
        town: Option<TownName>,
        journal: JournalName,
        text: String,
        description: Option<String>,
        cites: Vec<EntryId>,
        attach: Vec<Attach>,
        /// The writer's model, when the World does not know it.
        model: Option<String>,
    },
    /// Writes the next version of a journal's description.
    Describe {
        town: Option<TownName>,
        journal: JournalName,
        text: String,
    },
    /// The latest entries that fit in one answer, before `before` when given.
    Read {
        town: Option<TownName>,
        journal: JournalName,
        before: Option<EntryId>,
    },
    List {
        town: Option<TownName>,
    },
}

/// A file to attach to an entry: on the machine the command runs on, or on
/// `machine`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attach {
    pub machine: Option<MachineName>,
    pub path: String,
}
