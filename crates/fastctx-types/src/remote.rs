//! What one machine asks another to do, and what it sends a resident who
//! lives there (`world-network.md`, 四种话).

use crate::agents::AgentsReply;
use crate::management::{Managed, Management};
use serde::{Deserialize, Serialize};

use crate::access::Purpose;

use crate::JobId;
use crate::copy::{CopyReport, Place};
use crate::ipc::Failure;
use crate::project::CanonHead;
use crate::residents::{Landed, Notice};
use crate::run::{RunReply, RunRequest};
use crate::session::control::{Control, Controlled, Offers};
use crate::town::{Looked, WhoReply};
use crate::world::MachineFacts;
use crate::write::{WriteOp, WriteOutcome};
use crate::{AgentName, ResidentId, TownName};

/// A call: asked of a machine that is online, answered at once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "call")]
pub enum RemoteCall {
    /// The actual directory this machine would use for this conversation.
    Scratch {
        owner: crate::Address,
    },
    /// A running job returns a plan operation to its resident's home through the hub.
    Plan {
        resident: ResidentId,
        operation: crate::plan::Operation,
    },
    Collaboration {
        resident: ResidentId,
        colleagues: bool,
    },
    /// Replica paths on an account's own computer, without any document bytes.
    MemoryFolders {
        town: TownName,
    },
    Goal {
        resident: ResidentId,
        request: crate::goals::Request,
    },
    /// A node forwards a resident command, including commands from its running jobs,
    /// to the home that owns the goal's durable effects.
    GoalCommand {
        actor: crate::Address,
        turn: Option<String>,
        request: crate::goals::AgentRequest,
    },
    ConversationDebt {
        resident: ResidentId,
        requester: crate::Address,
    },
    WaitingJobs {
        owner: crate::Address,
    },
    SentinelControl {
        request: crate::sentinel_control::Request,
    },
    SentinelHome {
        request: crate::sentinel_control::HomeRequest,
    },
    SentinelSettings {
        change: Option<crate::sentinel_settings::WriteSettings>,
    },
    /// Keeps or removes one of the account's sentinel keys on its home.
    SentinelKey {
        key: crate::sentinel_settings::KeyName,
        value: Option<crate::sentinel_settings::Secret>,
    },
    /// Which of the account's sentinel keys its home holds.
    SentinelKeys,
    /// A running job returns its resident's project selection to its home.
    /// This call is relayed and authorized by a hub supporting project entry.
    EnterProject {
        resident: ResidentId,
        town: TownName,
    },
    /// Where a file attached for `target` may be kept, the machine asked
    /// being where the conversation lives.
    AttachmentPlaces {
        target: crate::attachments::UploadTarget,
        preferred: Option<crate::access::Written>,
    },
    Mcp {
        mcp: crate::mcp::MachineCall,
    },
    /// A dedicated bidirectional JSON-RPC connection, owned by this call.
    McpServe {
        name: String,
    },
    /// The user's shared composer, addressed directly to its resident machine.
    Composer {
        resident: ResidentId,
        request: crate::composer::Request,
    },
    /// A page action admitted by a machine's user-only page gate. Devices
    /// use Relay(Page::Act), never this execution call.
    PageAction {
        source: crate::pages::PageSource,
        action: crate::pages::PageInvocation,
    },
    /// Run a script there for a resident of the calling machine that works
    /// there (`world-run.md`, 这里): a job of that machine's, answered as
    /// it would be answered there.
    Run {
        resident: ResidentId,
        town: Option<TownName>,
        request: RunRequest,
        /// The caller acknowledges host receipt and deduplicates completion mail.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        answer_receipts: bool,
        /// Unique to this dispatch, so a delayed stop cannot interrupt a later turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dispatch: Option<crate::NoticeId>,
    },
    /// Words came for a resident of the calling machine: a `Run` of it
    /// waiting there answers now, as one here would.
    WordsCame {
        resident: ResidentId,
    },
    /// A turn of a resident of the calling machine was interrupted: the job
    /// a `Run` of it waits for there stops, as one here would.
    TurnInterrupted {
        resident: ResidentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dispatch: Option<crate::NoticeId>,
    },
    /// The answer to a `Run` of a resident of the calling machine showed its
    /// ended job whole, and its host has it: the job's files go.
    Forget {
        resident: ResidentId,
        town: Option<TownName>,
        job: JobId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery: Option<crate::run::JobDelivery>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<crate::on::OnOrigin>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        through_exit: bool,
    },
    /// Run a part of a script there (`world-run.md`, `on`). The package
    /// follows on the data connection, `sizes` long; then the machine says
    /// `OnWord`s and the job's output on it until the job ends.
    On {
        origin: crate::on::OnOrigin,
        sizes: crate::on::OnSizes,
        /// The World's write commands are off in the sending shell.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        writes_off: bool,
        /// The job's standard output stays there for the machine of the
        /// `on` that reads it to take, and does not come back.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        piped: bool,
        /// The job's standard input is the output of this `on`, which the
        /// machine takes from where it is kept before the job starts.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<crate::on::OnSource>,
    },
    /// Hand over, on this call's data connection, the standard output of
    /// the `on` job `job`, which went straight on to the calling machine
    /// (`world-run.md`, 数据跟着命令走), as `PipeWord`s and the bytes after
    /// them, until it is all there.
    PipeOut {
        origin: crate::on::OnOrigin,
        job: JobId,
    },
    /// Go on handing over an `on` job's output from these byte offsets, on
    /// the data connection, after the one it started on broke.
    Follow {
        origin: crate::on::OnOrigin,
        job: JobId,
        out_from: u64,
        err_from: u64,
    },
    /// Stop a job there and everything it started, for `by` on the calling
    /// machine (`fastctx stop`).
    StopJob {
        job: JobId,
        by: String,
    },
    /// Stop an `on` job: its sender stopped it on purpose.
    StopOn {
        origin: crate::on::OnOrigin,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job: Option<JobId>,
    },
    /// The resident's machine arbitrates a nested completion using the
    /// durable receipt kept by the machine whose CLI followed that hop.
    OnReceipt {
        owner: crate::Address,
        machine: crate::MachineName,
        job: JobId,
        request: Option<crate::NoticeId>,
    },
    /// Write files there for a resident of the calling machine.
    Write {
        resident: ResidentId,
        town: Option<TownName>,
        op: WriteOp,
    },
    /// Who is on that machine and what each is doing, for `fastctx w`: in
    /// `town`, or in every town and none.
    Who {
        all: bool,
        town: Option<TownName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<jiff::Timestamp>,
    },
    /// Existing local records strictly after this timestamp. Large replies
    /// use the telephone's existing authenticated data connection.
    Since {
        since: jiff::Timestamp,
        town: Option<TownName>,
    },
    /// Facts from this machine's official agent programs.
    Agents,
    /// A user's byte read. `read` binds the signed data pieces to this call.
    ReadFile {
        request: crate::file_read::ReadFileRequest,
        read: u64,
    },
    Thumbnail {
        request: crate::thumbnail::ThumbnailRequest,
    },
    AttachmentThumbnail {
        attachment: crate::attachments::AttachmentSource,
        max: u32,
    },
    ReadAttachment {
        attachment: crate::attachments::AttachmentSource,
        read: u64,
        #[serde(default, skip_serializing_if = "crate::file_read::FileRange::is_all")]
        range: crate::file_read::FileRange,
    },
    Lines {
        request: crate::lines::Request,
    },
    ReadLines {
        request: crate::lines::ReadRequest,
        read: u64,
    },
    ReadLineNotes {
        request: crate::lines::ReadRequest,
        read: u64,
    },
    FindFiles {
        request: crate::file_search::FileSearchRequest,
    },
    /// The folders in a folder there, for the user to choose one.
    Folders {
        path: Option<String>,
    },
    /// What each of these folders there holds, one level deep, read within
    /// `within_ms`, for a conversation pointed at them.
    Outline {
        folders: Vec<String>,
        within_ms: u64,
    },
    /// The user's word on Claude Code's bypass-permissions consent there.
    ClaudeConsent {
        accept: bool,
        /// The Claude Code home there it is for, when not the default one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<std::path::PathBuf>,
    },
    /// The user adds an agent's home there, or hides one the World found
    /// or shows it again.
    Homes {
        change: crate::agents::HomeChange,
    },
    /// What that machine's node keeps of its agents' logins.
    AgentLogins,
    /// Scan that machine for its agents, or stop the searches of its
    /// chosen folders; only for the machine's owner.
    ScanAgents {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stop: bool,
    },
    /// Take up a directory there as a candidate login; only for the
    /// machine's owner.
    AddAgentLogin {
        agent: AgentName,
        dir: std::path::PathBuf,
    },
    /// Choose a folder there for logins, stop its search, or no longer
    /// search it; only for the machine's owner.
    AgentFolder {
        dir: std::path::PathBuf,
        change: crate::logins::FolderChange,
    },
    /// Turn the automatic naming of new Codex conversations in a login there
    /// on or off; only for the machine's owner.
    SetAgentTitle {
        agent: AgentName,
        dir: std::path::PathBuf,
        on: bool,
    },
    /// What a change of a login there would do; only for the machine's
    /// owner.
    PreviewAgentChange {
        agent: AgentName,
        dir: std::path::PathBuf,
        into: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        take_back: bool,
    },
    /// Put the World's tools into a login there, or take them out; only for
    /// the machine's owner.
    ChangeAgentLogin {
        agent: AgentName,
        dir: std::path::PathBuf,
        into: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        raise_output_limit: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        take_back: bool,
    },
    /// Read a login's limits there afresh.
    ReadAgentLimits {
        agent: AgentName,
        dir: std::path::PathBuf,
    },
    /// What a login there was used for: everyone's to an account that may
    /// work there, its own alone to one the login assists.
    AgentUsage {
        agent: AgentName,
        dir: std::path::PathBuf,
    },
    /// Up to one hundred request records, newest first, redacted for this reader.
    AgentCalls {
        agent: AgentName,
        dir: std::path::PathBuf,
        query: crate::usage_calls::UsageCallsQuery,
    },
    /// The documents of a canon there, for a town's map.
    Canon {
        path: String,
    },
    /// Start a colleague there for a resident of the calling machine,
    /// working for the one account named by the call's envelope.
    Ask {
        asker: ResidentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requester_turn: Option<String>,
        town: Option<TownName>,
        label: String,
        prompt: String,
        started: crate::collab::Started,
    },
    /// Whether a resident with this id lives there.
    Find {
        resident: ResidentId,
    },
    /// A resident or job of that machine in detail, for `fastctx w <id>`.
    Look {
        id: String,
    },
    /// The user's words, typed on the calling machine, to a resident there.
    Talk {
        to: ResidentId,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<crate::attachments::Attachment>,
        /// Where the attached files other than images go, when the user
        /// chose.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        files_on: Option<crate::access::Written>,
        /// This device's name for these words: said again under it, as
        /// after an answer that never came, they go in once.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Whether each stored attachment still has a live conversation reference at its home.
    AttachmentReferences {
        conversation: crate::Address,
        attachments: Vec<crate::attachments::Attachment>,
    },
    /// Reserve metadata at the conversation's home; bytes stay with the caller.
    ReserveAttachment {
        target: crate::attachments::UploadTarget,
        file: crate::attachments::UploadFile,
        operation: String,
        composer: Option<crate::composer::UploadScope>,
    },
    /// Complete a reserved upload with its verified location and small history image.
    CommitAttachment {
        attachment: crate::attachments::Attachment,
        preview: Option<crate::thumbnail::Thumbnail>,
    },
    /// Keep here a file the user attached to their words for `resident`, of
    /// the calling machine, which works here (`world-gui.md`): its receipt
    /// where it lives comes with the call, and over the call's data
    /// connection this machine says whether it needs the bytes, which then
    /// come, followed by the JSON word "placed".
    Place {
        resident: ResidentId,
        attachment: crate::attachments::Attachment,
    },
    /// Bytes arrive through this signed call's data connection, followed by
    /// the JSON word "uploaded". Only a user-authorized entrance emits this.
    Upload {
        target: crate::attachments::UploadTarget,
        file: crate::attachments::UploadFile,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        composer: Option<crate::composer::UploadScope>,
        /// The caller skips what this machine already holds of an upload
        /// that broke: this machine first says an `UploadStart`, and the
        /// bytes from there on follow. Said only to a machine whose facts
        /// say `upload_resumes`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        resumes: bool,
    },
    /// Watch a resident there: what it hands over goes over the call's data
    /// connection until the caller closes it, each piece signed by the
    /// machine that holds the session, naming `watch` and numbered from 0.
    Watch {
        /// Requests the multiplexed watch protocol. A peer that supports it
        /// answers with a signed Ready packet; an older peer sends its ordinary
        /// watch, which remains the first subscription's complete transport.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        multiplex: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        goal: bool,
        resident: ResidentId,
        watch: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        composer: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        documents: Option<crate::composer::sync::Subscription>,
        /// What the viewer keeps of the history (`session::held`). Said, the
        /// pieces are signed as zstd-compressed JSON (`fastctx-net`'s
        /// `history::pack`) and the history comes as segments.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held: Option<crate::session::held::Held>,
        /// The viewer reads `session::Watched::Now`; a machine from before
        /// it reads past this and sends none.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        now: bool,
    },
    /// Open a session there for the user, as `Call::Open`.
    Open {
        #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
        mcp_off: std::collections::BTreeSet<String>,
        agent: AgentName,
        town: Option<TownName>,
        text: String,
        model: Option<String>,
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<crate::attachments::Attachment>,
        #[serde(default, skip_serializing_if = "crate::focus::Focus::is_empty")]
        focus: crate::focus::Focus,
        /// The agent's home there it uses, when not the default one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<std::path::PathBuf>,
        /// Where the files attached to its first words go, other than
        /// images, when the user chose.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        files_on: Option<crate::access::Written>,
    },
    /// Stop the turn a resident there is running, for the user.
    Stop {
        resident: ResidentId,
    },
    /// Do something to a resident's conversation there, for the user.
    Control {
        resident: ResidentId,
        control: Control,
        /// Reads a nested control result, whose fields cannot collide with the reply tag.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reply_envelope: bool,
    },
    /// Manage this resident as the user, on its own machine.
    Manage {
        resident: ResidentId,
        action: Management,
    },
    /// What a resident's conversation there offers now.
    Offers {
        resident: ResidentId,
    },
    /// The host's own record of one item of a resident's conversation there
    /// (`Call::ItemDetail`).
    ItemDetail {
        resident: ResidentId,
        turn: Option<String>,
        item: String,
    },
    /// What one of the host's own agents in a resident's conversation there
    /// did (`Call::AgentActivity`).
    AgentActivity {
        resident: ResidentId,
        agent: String,
    },
    /// What that machine is like, for `fastctx nodes`.
    Facts,
    /// Update that machine's World program from the newest release on its
    /// line, as `fastctx update` there does: only its owner's to ask, and the
    /// super administrator's but for a visitor's machine. What releases
    /// before 1.0.1 beta 3 take; later ones take `Updates`.
    UpdateWorld,
    /// New versions of Fastctx on that machine, as its own interface asks
    /// them (`crate::update::UpdateCall`), but for what only an interface
    /// there says or brings: who may update it may ask.
    Updates {
        update: crate::update::UpdateCall,
    },
    /// How that machine is doing, for its details page: only for an account
    /// that may work on it (`crate::stats`). With `after`, its lines bring
    /// only the round trips and changes since then: a page that asks every
    /// few seconds keeps what it was told.
    Stats {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<jiff::Timestamp>,
        /// What the last reading told, to bring only what came after it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seen: Option<crate::stats::StatsSeen>,
    },
    /// That machine's network addresses, as a place for the hub
    /// (`crate::hub_move`). A machine that cannot take the hub does not
    /// know this call.
    Interfaces,
    /// Copy `from`, a path there, to another machine.
    CopySend {
        from: String,
        to: Place,
    },
    /// Take a copy from the calling machine, over the call's data
    /// connection: first the manifest, whose size and digest are these, then
    /// the files the receiver asks for. `to` ends in a separator to copy
    /// into that directory.
    CopyReceive {
        to: String,
        manifest_bytes: u64,
        manifest_sha256: String,
        /// The source sends a file from a byte the destination names, so a
        /// copy that broke goes on from where it was.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        resumes: bool,
    },
    /// Send `from`, on this machine, to the calling machine over the call's
    /// data connection: first how long the manifest is, then the manifest,
    /// then the files the caller asks for. The caller is the destination of
    /// a copy, so what leaves here goes only to a member whose scope reaches
    /// this machine.
    CopyFetch {
        from: String,
    },
    /// A device's call that asks of the whole World (who is in it, what
    /// happened since, its projects), made by this machine's node for the
    /// user holding the device, as for a user at this machine. `made` is a
    /// local `Call`, as JSON; what a device asks of one resident it asks of
    /// that resident's machine itself.
    Relay {
        made: serde_json::Value,
    },
}

impl RemoteCall {
    /// What the call asks of the machine it is made of (`world-network.md`,
    /// 共享机器与借出智能体): the caller names it, the hub and the machine
    /// check it.
    pub fn purpose(&self) -> Purpose {
        match self {
            Self::Goal { request: crate::goals::Request::Read | crate::goals::Request::History { .. }, .. } => {
                Purpose::Look
            }
            Self::GoalCommand { request: crate::goals::AgentRequest::Read, .. } => Purpose::Look,
            Self::Goal { .. } | Self::GoalCommand { .. } => Purpose::Agents,
            Self::EnterProject { .. } | Self::Plan { .. } => Purpose::Echo,
            Self::SentinelSettings { .. } | Self::SentinelControl { .. } | Self::SentinelKeys => Purpose::Look,
            Self::SentinelKey { .. } => Purpose::Hands,
            Self::SentinelHome { request: crate::sentinel_control::HomeRequest::Inspect { .. } } => Purpose::Look,
            Self::SentinelHome { .. } => Purpose::Hands,
            RemoteCall::PageAction { action: crate::pages::PageInvocation::Say { .. }, .. } => Purpose::Words,
            RemoteCall::ReadAttachment { attachment: crate::attachments::AttachmentSource::Upload(file), .. }
                if file.owner != *file.conversation() =>
            {
                Purpose::Hands
            }
            RemoteCall::MemoryFolders { .. }
            | RemoteCall::Scratch { .. }
            | RemoteCall::Mcp { .. }
            | RemoteCall::McpServe { .. }
            | RemoteCall::Run { .. }
            | RemoteCall::WordsCame { .. }
            | RemoteCall::TurnInterrupted { .. }
            | RemoteCall::Forget { .. }
            | RemoteCall::Write { .. }
            | RemoteCall::On { .. }
            | RemoteCall::OnReceipt { .. }
            | RemoteCall::Follow { .. }
            | RemoteCall::StopOn { .. }
            | RemoteCall::StopJob { .. }
            | RemoteCall::FindFiles { .. }
            | RemoteCall::Folders { .. }
            | RemoteCall::Outline { .. }
            | RemoteCall::ReadFile { .. }
            | RemoteCall::Thumbnail { .. }
            | RemoteCall::Lines { .. }
            | RemoteCall::ReadLines { .. }
            | RemoteCall::ReadLineNotes { .. }
            | RemoteCall::Canon { .. }
            | RemoteCall::CopySend { .. }
            | RemoteCall::CopyFetch { .. }
            | RemoteCall::CopyReceive { .. }
            | RemoteCall::Place { .. }
            | RemoteCall::PipeOut { .. }
            | RemoteCall::PageAction { .. }
            | RemoteCall::ClaudeConsent { .. }
            | RemoteCall::Homes { .. }
            | RemoteCall::ScanAgents { .. }
            | RemoteCall::AddAgentLogin { .. }
            | RemoteCall::AgentFolder { .. }
            | RemoteCall::SetAgentTitle { .. }
            | RemoteCall::PreviewAgentChange { .. }
            | RemoteCall::ChangeAgentLogin { .. }
            | RemoteCall::UpdateWorld
            | RemoteCall::Updates { .. }
            | RemoteCall::Stats { .. } => Purpose::Hands,
            RemoteCall::Collaboration { .. }
            | RemoteCall::AttachmentPlaces { .. }
            | RemoteCall::ReserveAttachment { .. }
            | RemoteCall::CommitAttachment { .. }
            | RemoteCall::Composer { .. }
            | RemoteCall::AttachmentThumbnail { .. }
            | RemoteCall::ReadAttachment { .. }
            | RemoteCall::Ask { .. }
            | RemoteCall::Talk { .. }
            | RemoteCall::Watch { .. }
            | RemoteCall::Open { .. }
            | RemoteCall::Stop { .. }
            | RemoteCall::Manage { .. }
            | RemoteCall::Control { .. }
            | RemoteCall::Offers { .. }
            | RemoteCall::ItemDetail { .. }
            | RemoteCall::AgentActivity { .. }
            | RemoteCall::Upload { .. } => Purpose::Agents,
            RemoteCall::Agents
            | RemoteCall::AgentLogins
            | RemoteCall::ReadAgentLimits { .. }
            | RemoteCall::AgentUsage { .. }
            | RemoteCall::AgentCalls { .. }
            | RemoteCall::ConversationDebt { .. }
            | RemoteCall::WaitingJobs { .. }
            | RemoteCall::Look { .. }
            | RemoteCall::Find { .. }
            | RemoteCall::Who { .. }
            | RemoteCall::Since { .. }
            | RemoteCall::Facts
            | RemoteCall::AttachmentReferences { .. }
            | RemoteCall::Interfaces
            | RemoteCall::Relay { .. } => Purpose::Look,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum RemoteReply {
    Scratch {
        path: String,
    },
    Plan {
        result: crate::plan::Result,
    },
    Collaboration(Box<crate::collaboration::Tree>),
    MemoryFolders(crate::memory::Folders),
    Goal {
        updated: Box<crate::goals::Updated>,
    },
    ConversationDebt {
        debt: crate::turn_evidence::ConversationDebt,
    },
    WaitingJobs {
        jobs: Vec<crate::turn_evidence::WaitHandle>,
    },
    SentinelControl(Box<crate::sentinel_control::Reply>),
    SentinelHome(Box<crate::sentinel_control::HomeReply>),
    SentinelSettings(Box<crate::sentinel_settings::SettingsReply>),
    SentinelKeys(crate::sentinel_settings::KeyStatus),
    Entered {
        town: TownName,
        map: Box<crate::project::MapView>,
    },
    Mcp {
        result: crate::mcp::MachineReply,
    },
    ComposerUploaded(Box<crate::composer::UploadResult>),
    Composer(Box<crate::composer::Updated>),
    PageAction {
        result: crate::pages::PageResult,
    },
    FilesFound(crate::file_search::FileSearchReply),
    Folders(crate::focus::Folders),
    Outlined {
        outlines: Vec<crate::focus::Outlined>,
    },
    Thumbnail(crate::thumbnail::Thumbnail),
    Lines(crate::lines::Reply),
    ClaudeConsent {
        accepted: bool,
        changed: bool,
    },
    AttachmentPlaces(crate::attachments::AttachmentPlaces),
    Uploaded(crate::attachments::Attachment),
    Run(Box<RunReply>),
    /// An `on` job's end was handed over, or it was stopped.
    OnDone,
    OnReceipt {
        taken: bool,
        following: bool,
    },
    /// The job's record once stopped, or as it was when it had ended already;
    /// `None` when this machine has no job with that id.
    JobStopped {
        record: Option<Box<crate::run::JobRecord>>,
    },
    Written {
        outcome: WriteOutcome,
    },
    Who(WhoReply),
    Since(crate::since::SinceReply),
    Agents(AgentsReply),
    AgentLogins(Box<crate::logins::MachineLogins>),
    AgentUsage(Box<crate::usage::UsageReport>),
    AgentCalls(Box<crate::usage_calls::UsageCallsPage>),
    AgentChangePreview(Box<crate::logins::ChangePreview>),
    AgentLoginChanged(Box<crate::logins::LoginChange>),
    FileRead {
        result: crate::file_read::FileReadResult,
    },
    Asked {
        colleague: ResidentId,
    },
    Found {
        lives_here: bool,
    },
    Looked {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        read_at: Option<jiff::Timestamp>,
        looked: Option<Looked>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        jobs: Vec<crate::town::RunningJob>,
    },
    Said {
        landed: Landed,
    },
    Opened {
        resident: ResidentId,
        landed: Landed,
    },
    Stopped {
        stopped: bool,
    },
    Managed(Managed),
    Controlled(Controlled),
    ControlResult {
        controlled: Controlled,
    },
    Offers(Box<Offers>),
    ItemDetail {
        detail: Option<serde_json::Value>,
    },
    AgentActivity {
        activity: Box<crate::session::AgentActivity>,
    },
    /// A watch ended: the caller left, or the session's machine stopped it.
    Watched,
    /// A conversation's setting there changed as asked.
    Set,
    Facts(MachineFacts),
    AttachmentReferences {
        used: Vec<bool>,
    },
    Stats(Box<crate::stats::MachineStats>),
    /// How updates stand on that machine after it was asked to update.
    Update(Box<crate::update::State>),
    Interfaces {
        addresses: Vec<crate::hub_move::InterfaceAddress>,
    },
    Copied(CopyReport),
    Canon {
        heads: Vec<CanonHead>,
    },
    /// The answer to a relayed call: the local `Reply`, as JSON.
    Relayed {
        answer: serde_json::Value,
    },
}

/// The answer to a call, as the called machine gave it.
pub type RemoteResult = Result<RemoteReply, Failure>;

/// A letter: something for a resident of another machine, delivered when
/// that machine is reachable, however long that takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "letter")]
pub enum Letter {
    /// Durable job controls, sent only to peers advertising control_letters.
    Control {
        command: Box<crate::delivery_control::ControlLetter>,
    },
    /// An authenticated result of a durable control request.
    ControlResult {
        request: crate::NoticeId,
        result: Result<(), Failure>,
    },
    JobFacts {
        to: ResidentId,
        update: Box<crate::run_facts::JobFacts>,
    },
    /// One durable part of a letter that exceeds the control-frame budget.
    Fragment {
        id: crate::NoticeId,
        index: usize,
        total: usize,
        text: String,
    },
    /// Something the World tells the resident `to`.
    Notice {
        to: ResidentId,
        notice: Box<Notice>,
    },
    /// Words from the resident `from` of the sending machine to the resident
    /// `to`, who is owed nothing; `from` is owed the end of the turn they go
    /// into. `accounts` are the others of this hub `from` works for, any of
    /// which `to` may work for; the letter is sent for one of them.
    Speak {
        to: ResidentId,
        from: ResidentId,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        accounts: Vec<crate::AccountName>,
    },
    /// The resident `resident` of the sending machine left: colleagues that
    /// work for it go to sleep once their turn ends.
    Left {
        resident: ResidentId,
    },
}
