//! Everything a local client can ask the node, and the answers.

use crate::agents::WorldAgents;
use crate::management::{Managed, Management};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::copy::{CopyReport, Place};
use crate::project::{Attachment, JournalSummary, JournalView, MapView, Record, RecordFields, Stamp, TownSummary};
use crate::residents::{Delivery, Hints, Host, Landed, Seen, Whereabouts};
use crate::run::{JobOwner, RunReply, RunRequest};
use crate::session::control::{Control, Controlled, Offers};
use crate::town::{LookedUp, WorldWho};
use crate::world::NodesReply;
use crate::write::{WriteOp, WriteOutcome};
use crate::{Address, AgentName, EntryId, JobId, JournalName, MachineName, NoticeId, ResidentId, TownName};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "call")]
pub enum Call {
    /// A session says who it is; the node finds or makes its resident.
    Arrive {
        host: Host,
        hints: Hints,
    },
    /// Run a script for a resident of this machine, on this machine or on
    /// the machine `node` names.
    Run {
        resident: ResidentId,
        request: RunRequest,
        node: Option<MachineName>,
    },
    /// Writes files for a resident of this machine, on this machine or on
    /// the machine `node` names.
    Write {
        resident: ResidentId,
        op: WriteOp,
        node: Option<MachineName>,
    },
    /// Copies files or a directory from one machine to another.
    Copy {
        from: Place,
        to: Place,
    },
    /// Reads a file's original bytes for the user, here or on `machine`.
    /// The connection carries `NodeFrame::FileRead` pieces until its reply.
    ReadFile {
        machine: Option<MachineName>,
        request: crate::file_read::ReadFileRequest,
    },
    /// File-name completion, with scope checked on the machine holding it.
    FindFiles {
        machine: Option<MachineName>,
        request: crate::file_search::FileSearchRequest,
    },
    /// Asks for a colleague for a resident of this machine
    /// (`ask_colleague`): a Codex resident started on this machine or on
    /// the machine `node` names, with `prompt` as its first words. `label`
    /// is the task's few words.
    Ask {
        resident: ResidentId,
        label: String,
        prompt: String,
        node: Option<MachineName>,
    },
    /// Words from a resident of this machine to another resident, on any
    /// machine (`message_colleague`); `machine` is where it lives, when the
    /// speaker said. The speaker is owed the end of the turn they go into.
    Message {
        resident: ResidentId,
        to: ResidentId,
        machine: Option<MachineName>,
        text: String,
    },
    /// Records what a resident saw of files on this machine through
    /// `fastctx view`; the resident may live on another machine.
    Saw {
        owner: Address,
        town: Option<TownName>,
        seen: Vec<Seen>,
    },
    /// Who is in a town and what each is doing, asking each machine it spans
    /// (`fastctx w`); without a town, the caller's, or the World as a whole
    /// for a caller in none. With `all`, also the residents who have left.
    Who {
        all: bool,
        town: Option<TownName>,
    },
    /// Existing records strictly after `since`, from every machine of `town`
    /// or the whole World when absent. No caller-town default is applied.
    Since {
        since: Timestamp,
        town: Option<TownName>,
    },
    /// Agent programs on one machine, or every online machine in the World.
    Agents {
        machine: Option<MachineName>,
    },
    /// The user's word, from one of the user's devices, on whether Claude
    /// Code on `machine` may run the World's sessions without approvals: its
    /// record that the user accepted the bypass-permissions dialog.
    ClaudeConsent {
        machine: MachineName,
        accept: bool,
    },
    /// One resident or job in detail, by its id, from every machine that
    /// has it (`fastctx w <id>`).
    Look {
        ids: Vec<String>,
    },
    /// The user's words to a resident on any machine (`fastctx talk`);
    /// `machine` is where it lives, when the user said.
    Talk {
        to: ResidentId,
        machine: Option<MachineName>,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<crate::attachments::Attachment>,
    },
    /// The user's byte stream, followed by an empty frame, on its own IPC
    /// connection. The final Uploaded reply is sent only after verification.
    Upload {
        target: crate::attachments::UploadTarget,
        node: Option<MachineName>,
        file: crate::attachments::UploadFile,
    },
    /// Watches a resident on any machine: the node answers with its history
    /// and then each event as it happens, for as long as the connection
    /// stays open.
    Watch {
        resident: ResidentId,
        machine: Option<MachineName>,
    },
    /// Opens a session for the user: a resident of `agent` started on this
    /// machine or on `node`, in `town`, with the user's words as its first.
    Open {
        agent: AgentName,
        town: Option<TownName>,
        node: Option<MachineName>,
        text: String,
        model: Option<String>,
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<crate::attachments::Attachment>,
    },
    /// Stops the turn a resident on any machine is running, for the user.
    Stop {
        resident: ResidentId,
        machine: Option<MachineName>,
    },
    /// Does something to a resident's conversation on any machine for the
    /// user, besides speaking into it (`session::control`).
    Control {
        resident: ResidentId,
        machine: Option<MachineName>,
        control: Control,
    },
    /// Manage this resident as the user, on its own machine.
    Manage {
        resident: ResidentId,
        machine: Option<MachineName>,
        action: Management,
    },
    /// What a resident's conversation on any machine offers now: the
    /// controls it takes, its commands and models, its usage.
    Offers {
        resident: ResidentId,
        machine: Option<MachineName>,
    },
    /// How to open a resident of this machine in its host's own interface,
    /// for the user (`fastctx attach`).
    Attach {
        resident: ResidentId,
    },
    /// What waits to be handed to a resident with the answer of a World
    /// tool, oldest first, with what it is to be told of where it is. It is
    /// handed to no other answer meanwhile.
    Collect {
        resident: ResidentId,
    },
    /// The named notices from `Collect`, and all its town notes, went out
    /// with the answer. Unnamed notices remain waiting; an empty list can
    /// acknowledge the town notes alone.
    Taken {
        resident: ResidentId,
        ids: Vec<NoticeId>,
    },
    /// `fastctx project`.
    Project(ProjectCall),
    /// `fastctx journal`.
    Journal(JournalCall),
    Page(crate::pages::PageCall),
    /// A supervisor says its job's record has changed: the shell started
    /// or exited, the output ended, or nothing of the job runs any more.
    JobChanged {
        job: JobId,
        owner: JobOwner,
        town: Option<TownName>,
    },
    /// Joins this machine to a World with a code, under `name` or the name
    /// it has.
    Join {
        code: String,
        name: Option<MachineName>,
    },
    /// Makes an invitation for one more machine, or for a device such as a
    /// phone, which carries the user's words and looks and runs nothing.
    Invite {
        name: Option<MachineName>,
        expires: Timestamp,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        device: bool,
    },
    /// Revokes a member's key, so no machine trusts it any more.
    Revoke {
        name: MachineName,
        reason: String,
    },
    /// This machine leaves its World and becomes a World of one again.
    Leave,
    /// The node hands the World over once idle and exits: a system service
    /// now keeps this World's node, and its node waits to take over.
    HandOver,
    /// This machine's link and every machine of the World (`fastctx nodes`).
    Nodes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum Reply {
    Page {
        view: Box<crate::pages::PageView>,
    },
    Pages {
        town: TownName,
        pages: Vec<crate::pages::PageSummary>,
    },
    PageHistory {
        town: TownName,
        page: crate::PageName,
        versions: Vec<crate::pages::PageVersion>,
    },
    PageConsent {
        town: TownName,
        page: crate::PageName,
        consent: crate::pages::PageConsent,
    },
    PageAct {
        result: crate::pages::PageResult,
    },
    FilesFound(crate::file_search::FileSearchReply),
    /// What Claude Code on `machine` now records; `changed` is false when it
    /// already said so.
    ClaudeConsent {
        machine: MachineName,
        accepted: bool,
        changed: bool,
    },
    Uploaded(crate::attachments::Attachment),
    Arrived(Whereabouts),
    Run(RunReply),
    Written {
        outcome: WriteOutcome,
    },
    Copied(CopyReport),
    FileRead {
        result: crate::file_read::FileReadResult,
    },
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
    /// The resident was told to stop its turn, or `stopped` is false: it
    /// was running none. Its turn ends as turns do, from its host.
    Stopped {
        resident: Address,
        stopped: bool,
    },
    Managed {
        resident: Address,
        managed: Managed,
    },
    Controlled {
        resident: Address,
        controlled: Controlled,
    },
    Offers {
        resident: Address,
        offers: Box<Offers>,
    },
    /// The command that opens the resident in its host's own interface,
    /// run in the user's terminal.
    Attachable(HostCommand),
    /// What waits for a resident, and lines for the head of its answer.
    Notices {
        notices: Vec<Delivery>,
        notes: Vec<String>,
    },
    Who(WorldWho),
    Since(crate::since::WorldSince),
    Agents(WorldAgents),
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

/// A command of an agent's own host, as the user runs it in a terminal:
/// what to start, with which arguments and which variables set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCommand {
    pub program: std::path::PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}
