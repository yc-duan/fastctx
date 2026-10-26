//! Everything a local client can ask the node, and the answers.

use crate::access::Written;

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
    Plan {
        hub: Option<crate::HubName>,
        account: Option<crate::AccountName>,
        operation: crate::plan::Operation,
    },
    PlanRead {
        hub: Option<crate::HubName>,
        account: Option<crate::AccountName>,
        owner: ResidentId,
    },
    PlanWrite {
        hub: Option<crate::HubName>,
        account: Option<crate::AccountName>,
        owner: ResidentId,
        expected: Option<u64>,
        markdown: String,
    },
    AccountRecords {
        hub: Option<crate::HubName>,
        account: Option<crate::AccountName>,
        request: crate::account_records::Request,
    },
    WatchAccountRecords {
        hub: Option<crate::HubName>,
        account: Option<crate::AccountName>,
        kind: String,
    },
    Collaboration {
        resident: ResidentId,
        machine: Option<crate::access::Written>,
    },
    Memory(crate::memory::MemoryCall),
    WatchMemory {
        town: TownName,
    },
    MemorySettings {
        hub: Option<crate::HubName>,
        account: Option<crate::AccountName>,
        enabled: Option<bool>,
    },
    Goal { resident: ResidentId, machine: Option<crate::access::Written>, account: Option<crate::access::HubAccount>, request: crate::goals::Request },
    GoalCommand { hub: Option<crate::HubName>, account: Option<crate::AccountName>, job: Option<crate::JobId>, request: crate::goals::AgentRequest },
    Timer { timer: crate::timers::TimerCall },
    TurnEvidence { resident: ResidentId, turn: Option<String>, refresh_limits: bool },
    CompactBetweenTurns { resident: ResidentId },
    Sentinel {
        hub: Option<crate::HubName>,
        account: Option<crate::AccountName>,
        request: crate::sentinel_control::Request,
    },
    SentinelSettings {
        hub: Option<crate::HubName>,
        account: Option<crate::AccountName>,
        change: Option<crate::sentinel_settings::WriteSettings>,
    },
    /// Hands one sentinel key to the account's sentinel home; `None` removes it.
    SentinelKey {
        hub: Option<crate::HubName>,
        account: Option<crate::AccountName>,
        key: crate::sentinel_settings::KeyName,
        value: Option<crate::sentinel_settings::Secret>,
    },
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
    /// The account's own layer (`world-user-config.md`) as `account` has it
    /// at `hub`; the one account the asking session works for, or this
    /// computer is logged in to, when none is named.
    /// A dedicated JSON-RPC stream for the calling account conversation.
    McpRelay {
        name: String,
    },
    /// Reads or changes an account's Claude subagents alone, over its
    /// profile's version.
    ProfileSubagents {
        hub: Option<crate::HubName>,
        account: Option<crate::AccountName>,
        operation: crate::profile::SubagentOperation,
    },
    ProfileRead {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hub: Option<crate::HubName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<crate::AccountName>,
    },
    /// Writes what `ProfileRead` reads, over the version it was read at. Its
    /// servers are written through `Mcp`, which keeps what a command server
    /// needs on its machine; this keeps the list as it was read.
    ProfileWrite {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hub: Option<crate::HubName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<crate::AccountName>,
        profile: crate::profile::Profile,
    },
    /// The account's MCP servers, chosen as `ProfileRead` chooses it.
    Mcp {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hub: Option<crate::HubName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<crate::AccountName>,
        mcp: crate::mcp::McpCall,
    },
    /// Which optional parts of the interface this computer shows; its
    /// graphical interface and its TUI share them (`crate::features`).
    Features,
    /// Turns one of them on or off; answered with all of them after.
    SetFeature {
        feature: crate::features::Feature,
        on: bool,
    },
    /// Says which language this device's interfaces speak, by its tag;
    /// answered with the switches after.
    SetLanguage {
        language: String,
    },
    /// Answered with `NodeFrame::Features`: the switches as they are, then
    /// again each time they change, until the client leaves.
    WatchFeatures,
    /// The World as an interface shows it, read from what this computer
    /// holds (`crate::world::WorldNow`).
    WorldNow,
    /// Answered with `NodeFrame::World`: the World as an interface shows it,
    /// then again each time it changes, until the client leaves.
    WatchWorld,
    /// Answered `Done` once a way from this computer to `machine` opens that
    /// was not there when asked: its line to the hub came back, the machine
    /// connected anew, or a direct link with it came up. An interface that
    /// waits to open again what broke asks it, and leaves once its wait is
    /// over (`world-network.md`, 断开不是结束).
    Reopened {
        machine: Written,
    },
    /// Reads or changes the shared composer on the resident's machine.
    Composer {
        resident: ResidentId,
        machine: Option<Written>,
        request: crate::composer::Request,
    },
    /// A session says who it is; the node finds or makes its resident.
    Arrive {
        host: Host,
        hints: Hints,
    },
    /// The Claude subagents of the account a paired Claude session works
    /// for, as its plugin registers them.
    ClaudeSubagents {
        session: String,
    },
    /// Reserves a Claude exchange directory before the mod writes through it.
    ClaudeSessionDirectory {
        session: String,
        previous: Option<String>,
        cwd: Option<String>,
        process: Option<crate::ProcessIdentity>,
        /// The conversation the World started the session for, as the
        /// settings it was started with name it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation: Option<String>,
    },
    /// Run a script for a resident of this machine, on this machine or on
    /// the machine `node` names.
    Run {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        resident: ResidentId,
        request: RunRequest,
        node: Option<Written>,
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
        node: Option<Written>,
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
        machine: Option<Written>,
        request: crate::file_read::ReadFileRequest,
    },
    Thumbnail {
        machine: Option<Written>,
        request: crate::thumbnail::ThumbnailRequest,
    },
    AttachmentThumbnail {
        attachment: crate::attachments::AttachmentSource,
        max: u32,
    },
    /// Reads a file the user sent, or a picture a session showed, as
    /// `ReadFile` does; `range` lets a read that broke off go on.
    ReadAttachment {
        attachment: crate::attachments::AttachmentSource,
        #[serde(default, skip_serializing_if = "crate::file_read::FileRange::is_all")]
        range: crate::file_read::FileRange,
    },
    Lines {
        machine: Option<Written>,
        request: crate::lines::Request,
    },
    ReadLines {
        machine: Option<Written>,
        request: crate::lines::ReadRequest,
    },
    ReadLineNotes {
        machine: Option<Written>,
        request: crate::lines::ReadRequest,
    },
    /// File-name completion, with scope checked on the machine holding it.
    FindFiles {
        machine: Option<Written>,
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
    /// The folders in a folder of a machine, for the user to choose one;
    /// with no `path`, the places a choice starts from.
    Folders {
        machine: Option<Written>,
        path: Option<String>,
    },
    /// How a machine, or this computer, is doing, for its details page;
    /// with `after`, its lines bring only what came since then.
    Stats {
        machine: Option<crate::access::Written>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<jiff::Timestamp>,
        /// What the last reading told, to bring only what came after it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seen: Option<crate::stats::StatsSeen>,
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
        machine: Option<Written>,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        login: Option<crate::collab::Caller>,
    },
    /// `message_user`: words of `resident` for the user (`world-voice.md`).
    /// Older nodes do not list the tool, so no session calls it there.
    Say {
        resident: ResidentId,
        message: String,
        status: crate::residents::TellStatus,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<crate::voice::Attachment>,
    },
    /// Tells what one of the World's own commands did, from inside the job
    /// whose script gave it (`run_facts::RunStep`); the node names the job's
    /// machine. Older nodes do not know it, and the command goes on.
    Stepped {
        job: crate::JobId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        town: Option<crate::TownName>,
        id: String,
        began: jiff::Timestamp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ended: Option<jiff::Timestamp>,
        did: crate::run_facts::StepDid,
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
        machine: Option<Written>,
    },
    /// The user's word, from one of the user's devices, on whether Claude
    /// Code on `machine` may run the World's sessions without approvals: its
    /// record that the user accepted the bypass-permissions dialog.
    ClaudeConsent {
        machine: Written,
        accept: bool,
        /// The Claude Code home there it is for, when not the default one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<std::path::PathBuf>,
    },
    /// The user adds an agent's home on `machine`, or hides one the World
    /// found there or shows it again.
    Homes {
        machine: Written,
        change: crate::agents::HomeChange,
    },
    /// What the nodes of one machine, or of every machine the asker may
    /// use, keep of their agents' logins (`logins`); no process starts but
    /// to ask again a login whose sign-in changed since it was asked.
    AgentLogins {
        machine: Option<Written>,
    },
    /// Scans `machine`, one of the user's own, for its agents: answered
    /// once the usual places are done, while the folders the user chose are
    /// searched again; or, with `stop`, stops those searches.
    ScanAgents {
        machine: Written,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stop: bool,
    },
    /// Takes up a directory on `machine` as a candidate login of `agent`,
    /// asking the agent who is logged in there: one a folder's search found.
    AddAgentLogin {
        machine: Written,
        agent: AgentName,
        dir: std::path::PathBuf,
    },
    /// A folder on `machine` the user chooses for logins, whose search
    /// stops, or which is no longer searched (`logins::FolderChange`).
    AgentFolder {
        machine: Written,
        dir: std::path::PathBuf,
        change: crate::logins::FolderChange,
    },
    /// Whether the World names a new Codex conversation in the login `dir`
    /// on `machine` the way the official client does; off by default, set by
    /// whoever may manage the login (`world-agent-logins.md`, 在哪个登录里干活).
    SetAgentTitle {
        machine: Written,
        agent: AgentName,
        dir: std::path::PathBuf,
        on: bool,
    },
    /// What putting the World's tools into a login on `machine` (`into`),
    /// or taking them out, would change, with nothing written; with
    /// `take_back`, what the user changed of them is rewritten too.
    PreviewAgentChange {
        machine: Written,
        agent: AgentName,
        dir: std::path::PathBuf,
        into: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        take_back: bool,
    },
    /// Puts the World's tools into a login on `machine`, listing it, or
    /// takes them out, removing it from the list; done by that machine's
    /// node. Codex's shared output limit is raised only with
    /// `raise_output_limit`; what the user changed of the World's tools is
    /// rewritten only with `take_back`.
    ChangeAgentLogin {
        machine: Written,
        agent: AgentName,
        dir: std::path::PathBuf,
        into: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        raise_output_limit: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        take_back: bool,
    },
    /// Reads a login's usage limits afresh on its machine.
    ReadAgentLimits {
        machine: Written,
        agent: AgentName,
        dir: std::path::PathBuf,
    },
    /// What a login on a machine was used for, as the user may see it.
    AgentUsage {
        machine: Written,
        agent: AgentName,
        dir: std::path::PathBuf,
    },
    /// Up to one hundred request records, newest first, redacted for this reader.
    AgentCalls {
        machine: Written,
        agent: AgentName,
        dir: std::path::PathBuf,
        query: crate::usage_calls::UsageCallsQuery,
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
        machine: Option<Written>,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<crate::attachments::Attachment>,
        /// Where the attached files other than images go, when the user
        /// chose; see `agents::Choices::files_on`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        files_on: Option<crate::access::Written>,
        /// This device's name for these words: said again under it, as
        /// after an answer that never came, they go in once.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// File destinations available before an attachment picker opens.
    AttachmentPlaces {
        target: crate::attachments::UploadTarget,
        machine: Option<Written>,
        #[serde(default)]
        account: Option<crate::access::HubAccount>,
        #[serde(default)]
        preferred: Option<Written>,
    },
    /// The user's byte stream, followed by an empty frame, on its own IPC
    /// connection. The final Uploaded reply is sent only after verification.
    Upload {
        target: crate::attachments::UploadTarget,
        node: Option<Written>,
        file: crate::attachments::UploadFile,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        composer: Option<crate::composer::UploadScope>,
    },
    /// Watches a resident on any machine: the node answers with its history
    /// and then each event as it happens, for as long as the connection
    /// stays open.
    Watch {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<crate::access::HubAccount>,
        resident: ResidentId,
        machine: Option<Written>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        composer: Option<String>,
        /// What this viewer keeps of the history (`session::held`): the
        /// history then comes as segments, only those it does not hold.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held: Option<crate::session::held::Held>,
        /// The viewer reads `session::Watched::Now`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        now: bool,
    },
    /// Opens a session for the user: a resident of `agent` started on this
    /// machine or on `node`, in `town`, with the user's words as its first.
    Open {
        agent: AgentName,
        town: Option<TownName>,
        node: Option<Written>,
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
        machine: Option<Written>,
    },
    /// Does something to a resident's conversation on any machine for the
    /// user, besides speaking into it (`session::control`).
    Control {
        resident: ResidentId,
        machine: Option<Written>,
        control: Control,
    },
    /// What the World keeps on this machine, and deleting it. Answered here,
    /// never by another machine.
    Data(crate::data::DataCall),
    /// New versions of Fastctx for this machine (`crate::update`).
    Update(crate::update::UpdateCall),
    /// Account-scoped update management on a capable hub.
    NetworkUpdates {
        hub: crate::HubName,
        account: crate::AccountName,
        request: crate::update::ViewCall,
    },
    /// Update `machine`'s World program from the newest release on its
    /// line; answered with how updates stand there. What interfaces before
    /// 1.0.1 beta 3 ask; later ones ask `MachineUpdates`.
    UpdateMachine {
        machine: crate::access::Written,
    },
    /// New versions of Fastctx on `machine`, this one or another of the
    /// network's (`crate::update::UpdateCall`); answered with how updates
    /// stand there.
    MachineUpdates {
        machine: crate::access::Written,
        update: crate::update::UpdateCall,
    },
    /// Manage this resident as the user, on its own machine.
    Manage {
        resident: ResidentId,
        machine: Option<Written>,
        action: Management,
    },
    /// Deletes this resident's conversation from every machine and device
    /// keeping something of it, the one it lives on first, as the user
    /// (`world-colleagues.md`, 用户在任何地方和居民说话).
    Forget {
        resident: ResidentId,
        machine: Written,
    },
    /// What a resident's conversation on any machine offers now: the
    /// controls it takes, its commands and models, its usage.
    Offers {
        resident: ResidentId,
        machine: Option<Written>,
    },
    /// The host's own record of one item of a resident's conversation, as
    /// its host keeps it: what a slimmed history leaves out
    /// (`session::held`), asked for when the user would see it. A user's
    /// words are named by their message id, with no turn.
    ItemDetail {
        resident: ResidentId,
        machine: Option<Written>,
        turn: Option<String>,
        item: String,
    },
    /// What one of the host's own agents in a resident's conversation did,
    /// as its host keeps it, asked for when the user opens it: the agent's
    /// own record, which a conversation's history leaves out.
    AgentActivity {
        resident: ResidentId,
        machine: Option<Written>,
        agent: String,
    },
    /// How to open a resident of this machine in its host's own interface,
    /// for the user (`fastctx attach`).
    Attach {
        resident: ResidentId,
    },
    /// Keeps a local official host interface attached until this connection
    /// closes. Advertised by `HostCommand::hold`; older callers omit it.
    HoldHost {
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
    /// What the token `token` lets this computer into, asked of its hub
    /// before it is used: whose, which network, whether as a managed
    /// machine; and whether this computer may come in that way.
    PeekToken {
        token: String,
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
    /// Renames this computer `to`: on no hub, it is its own to name; on
    /// hubs, the one it is registered on renames it, and the others follow
    /// (`world-network.md`, 机器的名字).
    RenameHere {
        to: MachineName,
    },
    /// The HTTP proxy this computer reaches its hubs through, chosen in
    /// Fastctx: read it, or with `change`, set it to `url`, none clearing it.
    HubProxy {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        change: bool,
    },
    /// An enrollment token that registers a Linux server, or a Windows
    /// computer, as a machine of `account`, and the line that installs it
    /// there with this node's version.
    ServerCommand {
        hub: Option<crate::HubName>,
        account: crate::AccountName,
        name: Option<MachineName>,
        expires: Timestamp,
        /// The server becomes a managed machine of the account
        /// (`world-network.md`, 受管机器).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        managed: bool,
        /// How many servers may run the line, one when none, any number at
        /// zero; each takes its own name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uses: Option<u32>,
        /// What the code in it is called; for several servers, their group.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// The line is for Windows computers, not Linux servers.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        windows: bool,
        /// The line is for Macs, not Linux servers.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        mac: bool,
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
    /// The networks said dissolved were seen; they are said no more.
    SeenDissolved,
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
        /// The adapter renders and retains the per-conversation scratch paths.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        scratch: bool,
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
    Plan {
        result: crate::plan::Result,
    },
    AccountRecords {
        response: crate::account_records::Response,
    },
    Collaboration(Box<crate::collaboration::Tree>),
    Memory(crate::memory::MemoryReply),
    MemorySettings(crate::memory::Settings),
    Goal { updated: Box<crate::goals::Updated> },
    Timers { timers: Vec<crate::timers::Timer> },
    TurnEvidence { evidence: Box<crate::turn_evidence::TurnEvidence>, text: String },
    Compacted { result: crate::turn_evidence::Compacted },
    Sentinel(Box<crate::sentinel_control::Reply>),
    SentinelSettings(Box<crate::sentinel_settings::SettingsReply>),
    /// The HTTP proxy chosen in Fastctx, if one is (`Call::HubProxy`).
    HubProxy {
        url: Option<String>,
    },
    MachineLog(crate::machine_log::Page),
    CollabRead(crate::collab::CollabView),
    CollabWritten {
        version: u64,
    },
    Profile(Box<crate::mcp::ProfileView>),
    ProfileSubagents(Box<crate::profile::SubagentView>),
    ClaudeSubagents {
        configuration: Option<crate::profile::Subagents>,
    },
    ProfileWritten {
        version: u64,
    },
    Mcp {
        result: crate::mcp::McpReply,
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
    Folders(crate::focus::Folders),
    Stats(Box<crate::stats::MachineStats>),
    /// What Claude Code on `machine` now records; `changed` is false when it
    /// already said so.
    ClaudeConsent {
        machine: MachineName,
        accepted: bool,
        changed: bool,
    },
    AttachmentPlaces(crate::attachments::AttachmentPlaces),
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
        /// The words' id on the machine they went to, where the device that
        /// sent them named them (`Watched::Now`'s `kept`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery: Option<crate::NoticeId>,
    },
    /// What `Say` asked to tell the user is kept for them.
    Told,
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
    Forgetting {
        resident: Address,
        how: crate::management::Forgetting,
    },
    Data(crate::data::DataReply),
    Update(Box<crate::update::State>),
    NetworkUpdates(crate::update::ViewReply),
    ClaudeSessionDirectory {
        path: String,
    },
    Controlled {
        resident: Address,
        controlled: Controlled,
    },
    Offers {
        resident: Address,
        offers: Box<Offers>,
    },
    /// The host's record of the item asked for; none when its history now
    /// holds no such item.
    ItemDetail {
        resident: Address,
        detail: Option<serde_json::Value>,
    },
    AgentActivity {
        resident: Address,
        activity: Box<crate::session::AgentActivity>,
    },
    /// The command that opens the resident in its host's own interface,
    /// run in the user's terminal.
    Attachable(HostCommand),
    /// What waits for a resident, and lines for the head of its answer.
    Notices {
        notices: Vec<Delivery>,
        notes: Vec<String>,
        /// Notes before this index precede command output; the rest follow it.
        #[serde(default)]
        leading_notes: usize,
    },
    Who(WorldWho),
    Since(crate::since::WorldSince),
    Agents(WorldAgents),
    AgentLogins(crate::logins::WorldLogins),
    MachineAgentLogins(Box<crate::logins::MachineLogins>),
    AgentUsage(Box<crate::usage::UsageReport>),
    AgentCalls(Box<crate::usage_calls::UsageCallsPage>),
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
    /// What a token lets this computer into; none from a hub that says
    /// nothing of a token before it is used (its tokens are never for
    /// managed machines).
    TokenPeek {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peeked: Option<crate::accounts::TokenPeek>,
    },
    /// An account change was made; a token, when it made one.
    Changed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    Accounts(crate::accounts::AccountsReply),
    WorldNow(Box<crate::world::WorldNow>),
    ServerInvitation {
        command: String,
    },
    HubMove(crate::hub_move::HubMoveReply),
    Nodes(NodesReply),
    Machines {
        /// The machine the session lives on, as written, where the accounts
        /// it works for may work on it; None where they may not.
        here: Option<String>,
        /// The other machines it may work on.
        others: Vec<String>,
        /// Those of `others` their owners put in groups, group by group.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        groups: Vec<crate::world::MachineGroup>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        collab: Option<crate::collab::Offered>,
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        scratch: crate::scratch::Descriptions,
        /// Whether the session speaks to the user through `message_user`:
        /// one the World started for them (`world-voice.md`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        speaks: bool,
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

impl ProjectCall {
    /// The existing town the call names; `None` means the caller's, all of
    /// them, or a new one.
    pub fn town(&self) -> Option<&TownName> {
        match self {
            ProjectCall::New { .. } | ProjectCall::List => None,
            ProjectCall::Edit { town, .. } | ProjectCall::Show { town } | ProjectCall::Map { town } => town.as_ref(),
            ProjectCall::Enter { town } => Some(town),
        }
    }
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

impl JournalCall {
    /// The town the call names; `None` means the caller's.
    pub fn town(&self) -> Option<&TownName> {
        match self {
            JournalCall::Add { town, .. }
            | JournalCall::Describe { town, .. }
            | JournalCall::Read { town, .. }
            | JournalCall::List { town } => town.as_ref(),
        }
    }
}

/// A file to attach to an entry: on the machine the command runs on, or on
/// `machine`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attach {
    pub machine: Option<Written>,
    pub path: String,
}

/// A command of an agent's own host, as the user runs it in a terminal:
/// what to start, with which arguments and which variables set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCommand {
    /// The node supports a connection-scoped HoldHost for this conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<ResidentId>,
    pub program: std::path::PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}
