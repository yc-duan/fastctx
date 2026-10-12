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
    /// The selected account's machine changes, before a previous page's
    /// cursor when supplied. The hub checks visibility on every read.
    MachineLog {
        hub: crate::HubName,
        account: crate::AccountName,
        before: Option<u64>,
    },
    /// Model collaboration as `account` has it at `hub`, or at the one hub
    /// when none is named; with no account, this computer's own, which
    /// applies while it is in no network.
    CollabRead {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hub: Option<crate::HubName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<crate::AccountName>,
    },
    /// Writes what `CollabRead` reads, over the version it was read at.
    /// This computer's own is written only while it is in no network.
    CollabWrite {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hub: Option<crate::HubName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<crate::AccountName>,
        collab: crate::collab::Collab,
    },
    /// Which optional parts of the interface this computer shows; its
    /// graphical interface and its TUI share them (`crate::features`).
    Features,
    /// Turns one of them on or off; answered with all of them after.
    SetFeature {
        feature: crate::features::Feature,
        on: bool,
    },
    /// Answered with `NodeFrame::Features`: the switches as they are, then
    /// again each time they change, until the client leaves.
    WatchFeatures,
    /// Reads or changes the shared composer on the resident's machine.
    Composer {
        resident: ResidentId,
        machine: Option<MachineName>,
        request: crate::composer::Request,
    },
    /// A session says who it is; the node finds or makes its resident.
    Arrive {
        host: Host,
        hints: Hints,
    },
    /// Run a script for a resident of this machine, on this machine or on
    /// the machine `node` names.
    Run {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        resident: ResidentId,
        request: RunRequest,
        node: Option<MachineName>,
    },
    StoreRunFacts {
        resident: ResidentId,
        call_id: String,
        facts: Box<crate::run_facts::RunFacts>,
    },
    RecordRunFailure {
        resident: ResidentId,
        call_id: String,
        error: String,
    },
    /// Runs part of a script on another machine for `resident`, which may
    /// live elsewhere when this script is itself an `on` (`world-run.md`).
    /// The node answers with `NodeFrame::On` pieces as the job goes, then a
    /// `Reply`; the client closing the connection first stops the job.
    On {
        resident: Address,
        town: Option<TownName>,
        request: crate::on::OnRequest,
    },
    /// Writes for an MCP resident, or for the command's peer when `resident`
    /// is absent. Commands write here; their shell supplies a visitor's town.
    Write {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job: Option<crate::JobId>,
        resident: Option<ResidentId>,
        op: WriteOp,
        node: Option<MachineName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        town: Option<TownName>,
    },
    /// Stops a job and everything it started: on `machine`, or on the one
    /// machine that has a job with this id (`fastctx stop`).
    StopJob {
        job: JobId,
        machine: Option<MachineName>,
        /// The hub of `machine`, when it was written as `name.hub`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hub: Option<crate::HubName>,
    },
    /// Waits until jobs of this machine end (`fastctx wait`). The end of a
    /// job the caller started is taken by the wait: it is not told again.
    Wait {
        jobs: Vec<JobId>,
        /// The command acknowledges the rendered result before taking its ends.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        receipt: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent: Option<JobId>,
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
    Thumbnail {
        machine: Option<MachineName>,
        request: crate::thumbnail::ThumbnailRequest,
    },
    AttachmentThumbnail {
        attachment: crate::attachments::AttachmentSource,
        max: u32,
    },
    ReadAttachment {
        attachment: crate::attachments::AttachmentSource,
    },
    Lines {
        machine: Option<MachineName>,
        request: crate::lines::Request,
    },
    ReadLines {
        machine: Option<MachineName>,
        request: crate::lines::ReadRequest,
    },
    /// File-name completion, with scope checked on the machine holding it.
    FindFiles {
        machine: Option<MachineName>,
        request: crate::file_search::FileSearchRequest,
    },
    /// Asks for a colleague for a resident of this machine
    /// (`ask_colleague`): the named callee configured by one of its accounts,
    /// with `prompt` as its first words.
    /// `label` is the task's few words.
    Ask {
        resident: ResidentId,
        label: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        callee: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        login: Option<crate::collab::Caller>,
    },
    /// The user sets where a resident on any machine works: another
    /// machine, or the one it lives on with `None` (`world-run.md`, 这里).
    WorksOn {
        resident: ResidentId,
        machine: Option<MachineName>,
        works_on: Option<crate::access::Written>,
    },
    /// On a session's own connection: a `ToolsChanged` frame each time the
    /// tools `resident`, the session's resident when it is known, is
    /// offered may have changed, for as long as the connection stays open.
    WatchTools {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resident: Option<ResidentId>,
    },
    /// Words from a resident of this machine to another resident, on any
    /// machine (`message_colleague`); `machine` is where it lives, when the
    /// speaker said. The speaker is owed the end of the turn they go into.
    Message {
        resident: ResidentId,
        to: ResidentId,
        machine: Option<MachineName>,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        login: Option<crate::collab::Caller>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<jiff::Timestamp>,
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
        /// The Claude Code home there it is for, when not the default one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<std::path::PathBuf>,
    },
    /// The user adds an agent's home on `machine`, or hides one the World
    /// found there or shows it again.
    Homes {
        machine: MachineName,
        change: crate::agents::HomeChange,
    },
    /// What the nodes of one machine, or of every machine the asker may
    /// use, keep of their agents' logins (`logins`); no process starts.
    AgentLogins {
        machine: Option<MachineName>,
    },
    /// Scans `machine`, one of the user's own, for its agents: answered
    /// once the usual places are done, while the walk of every disk goes
    /// on; or, with `stop`, stops that walk.
    ScanAgents {
        machine: MachineName,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stop: bool,
    },
    /// Takes up a directory on `machine` as a candidate login of `agent`,
    /// asking the agent who is logged in there: one the user wrote, or one
    /// the walk of every disk found.
    AddAgentLogin {
        machine: MachineName,
        agent: AgentName,
        dir: std::path::PathBuf,
    },
    /// What putting the World's tools into a login on `machine` (`into`),
    /// or taking them out, would change, with nothing written.
    PreviewAgentChange {
        machine: MachineName,
        agent: AgentName,
        dir: std::path::PathBuf,
        into: bool,
    },
    /// Puts the World's tools into a login on `machine`, listing it, or
    /// takes them out, removing it from the list; done by that machine's
    /// node. Codex's shared output limit is raised only with
    /// `raise_output_limit`.
    ChangeAgentLogin {
        machine: MachineName,
        agent: AgentName,
        dir: std::path::PathBuf,
        into: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        raise_output_limit: bool,
    },
    /// Reads a login's usage limits afresh on its machine.
    ReadAgentLimits {
        machine: MachineName,
        agent: AgentName,
        dir: std::path::PathBuf,
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
        /// Where the attached files other than images go, when the user
        /// chose; see `agents::Choices::files_on`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        files_on: Option<crate::access::Written>,
    },
    /// The user's byte stream, followed by an empty frame, on its own IPC
    /// connection. The final Uploaded reply is sent only after verification.
    Upload {
        target: crate::attachments::UploadTarget,
        node: Option<MachineName>,
        file: crate::attachments::UploadFile,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        composer: Option<crate::composer::UploadScope>,
    },
    /// Watches a resident on any machine: the node answers with its history
    /// and then each event as it happens, for as long as the connection
    /// stays open.
    Watch {
        resident: ResidentId,
        machine: Option<MachineName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        composer: Option<String>,
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
        #[serde(flatten)]
        choices: crate::agents::Choices,
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
        /// The job this very answer is about, left out of the jobs the
        /// answer says still run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answering: Option<JobId>,
    },
    /// The named notices from `Collect`, and all its town notes, went out
    /// with the answer. Unnamed notices remain waiting; an empty list can
    /// acknowledge the town notes alone.
    Taken {
        resident: ResidentId,
        ids: Vec<NoticeId>,
        /// A job that ended within its call and whose output this answer
        /// showed whole: nobody reads its log or record again, and they go
        /// (`world-run.md`, 输出).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shown: Option<JobId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        completed: Option<crate::run::CompletedJob>,
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
    /// Logs this computer in to `account` on the hub at `hub` with its
    /// password, and registers it as one of the account's machines when
    /// `register` and the account may. A computer new to the hub asks for
    /// its own name there, or `name`.
    Login {
        hub: Vec<std::net::SocketAddr>,
        account: crate::AccountName,
        password: String,
        register: bool,
        name: Option<MachineName>,
    },
    /// Logs this computer in with an enrollment token.
    LoginWithToken {
        token: String,
        name: Option<MachineName>,
    },
    /// Opens `account` with `password` on a hub that lets anyone, then
    /// logs in to it as `Login` would.
    OpenAccount {
        hub: Vec<std::net::SocketAddr>,
        account: crate::AccountName,
        password: String,
        register: bool,
        name: Option<MachineName>,
    },
    /// Makes `change` as `account` on the hub named `hub`, or on the one
    /// hub this computer is on when `None`.
    Account {
        hub: Option<crate::HubName>,
        account: crate::AccountName,
        change: crate::accounts::AccountChange,
    },
    /// Every hub this computer is on, with what it sees of each.
    Accounts,
    /// An enrollment token that registers a Linux server as a machine of
    /// `account`, and the line that installs it there with this node's
    /// version.
    ServerCommand {
        hub: Option<crate::HubName>,
        account: crate::AccountName,
        name: Option<MachineName>,
        expires: Timestamp,
    },
    /// Moving the hub named `hub`, or the one hub, as `account`, its super
    /// administrator (`crate::hub_move`).
    HubMove {
        hub: Option<crate::HubName>,
        account: crate::AccountName,
        request: crate::hub_move::HubMoveRequest,
    },
    /// The system service of the hub `hub` moving to this computer is
    /// installed here (`fastctx hub arrive`): the old hub may reach it now.
    HubArrived {
        hub: crate::access::HubId,
    },
    /// The node hands the World over once idle and exits: a system service
    /// now keeps this World's node, and its node waits to take over.
    HandOver,
    /// Freeze submissions and disconnect the node before local uninstall.
    /// Only a local user command may request this; no node is started for it.
    UninstallPrepare,
    /// This computer's links and every machine its accounts may use
    /// (`fastctx nodes`).
    Nodes,
    /// The machines a session here may work on, as it writes them: this
    /// one, and the others its accounts may use, `name.hub` where a name
    /// is on more than one hub; for `resident`, the session's resident when
    /// it is known, also the collaboration its accounts offer it.
    Machines {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resident: Option<ResidentId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        login: Option<crate::collab::Caller>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum Reply {
    MachineLog(crate::machine_log::Page),
    CollabRead(crate::collab::CollabView),
    CollabWritten {
        version: u64,
    },
    Features(crate::features::Features),
    ComposerUploaded(Box<crate::composer::UploadResult>),
    Composer(Box<crate::composer::Updated>),
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
    Thumbnail(crate::thumbnail::Thumbnail),
    Lines(crate::lines::Reply),
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
    AgentLogins(crate::logins::WorldLogins),
    MachineAgentLogins(Box<crate::logins::MachineLogins>),
    AgentChangePreview(Box<crate::logins::ChangePreview>),
    AgentLoginChanged(Box<crate::logins::LoginChange>),
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
    /// This computer is the member `name` of the hub `hub`, logged in to
    /// `account`.
    LoggedIn {
        hub: crate::HubName,
        name: MachineName,
        account: crate::AccountName,
    },
    /// An account change was made; a token, when it made one.
    Changed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    Accounts(crate::accounts::AccountsReply),
    ServerInvitation {
        command: String,
    },
    HubMove(crate::hub_move::HubMoveReply),
    Nodes(NodesReply),
    Machines {
        /// Where the session's shell starts, as written: the machine it
        /// works on. None for an agent another account lent: it may not
        /// work where it runs, and each of its commands names its machine.
        here: Option<String>,
        /// The other machines it may work on.
        others: Vec<String>,
        /// The machine its own program runs on, when that is not `here` and
        /// it may work there too; one it may not work on is not named.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        program: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collab: Option<crate::collab::Offered>,
    },
    /// A job stopped, or found already ended: on `machine`, as its record
    /// now stands.
    JobStopped {
        machine: MachineName,
        record: Box<crate::run::JobRecord>,
    },
    /// The jobs a `Wait` named, as their records stood when each ended.
    Waited {
        records: Vec<crate::run::JobRecord>,
    },
    Done,
}

/// `fastctx project`: the towns, their records and maps, and entering one.
/// Without a town, a call means the caller's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum ProjectCall {
    /// Opens a town with the first version of its record, in `account` of
    /// `hub`, or in the one account whoever asks works for.
    New {
        town: TownName,
        fields: RecordFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<crate::AccountName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hub: Option<crate::HubName>,
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
