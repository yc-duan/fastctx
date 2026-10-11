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
    },
    /// Words came for a resident of the calling machine: a `Run` of it
    /// waiting there answers now, as one here would.
    WordsCame {
        resident: ResidentId,
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
    },
    Lines {
        request: crate::lines::Request,
    },
    ReadLines {
        request: crate::lines::ReadRequest,
        read: u64,
    },
    FindFiles {
        request: crate::file_search::FileSearchRequest,
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
    /// The documents of a canon there, for a town's map.
    Canon {
        path: String,
    },
    /// Start a colleague there for a resident of the calling machine,
    /// working for the one account named by the call's envelope.
    Ask {
        asker: ResidentId,
        town: Option<TownName>,
        label: String,
        prompt: String,
        started: crate::collab::Started,
    },
    /// The user sets where a resident there works: another machine, or
    /// the one it lives on.
    WorksOn {
        resident: ResidentId,
        works_on: Option<crate::access::Written>,
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
    },
    /// Watch a resident there: what it hands over goes over the call's data
    /// connection until the caller closes it, each piece signed by the
    /// machine that holds the session, naming `watch` and numbered from 0.
    Watch {
        resident: ResidentId,
        watch: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        composer: Option<String>,
    },
    /// Open a session there for the user, as `Call::Open`.
    Open {
        agent: AgentName,
        town: Option<TownName>,
        text: String,
        model: Option<String>,
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<crate::attachments::Attachment>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        works_on: Option<crate::access::Written>,
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
    /// What that machine is like, for `fastctx nodes`.
    Facts,
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
            RemoteCall::Run { .. }
            | RemoteCall::WordsCame { .. }
            | RemoteCall::Forget { .. }
            | RemoteCall::Write { .. }
            | RemoteCall::On { .. }
            | RemoteCall::OnReceipt { .. }
            | RemoteCall::Follow { .. }
            | RemoteCall::StopOn { .. }
            | RemoteCall::StopJob { .. }
            | RemoteCall::FindFiles { .. }
            | RemoteCall::ReadFile { .. }
            | RemoteCall::Thumbnail { .. }
            | RemoteCall::Lines { .. }
            | RemoteCall::ReadLines { .. }
            | RemoteCall::Canon { .. }
            | RemoteCall::CopySend { .. }
            | RemoteCall::CopyFetch { .. }
            | RemoteCall::CopyReceive { .. }
            | RemoteCall::Place { .. }
            | RemoteCall::PipeOut { .. }
            | RemoteCall::PageAction { .. }
            | RemoteCall::ClaudeConsent { .. }
            | RemoteCall::Homes { .. } => Purpose::Hands,
            RemoteCall::Composer { .. }
            | RemoteCall::AttachmentThumbnail { .. }
            | RemoteCall::ReadAttachment { .. }
            | RemoteCall::Ask { .. }
            | RemoteCall::Talk { .. }
            | RemoteCall::Watch { .. }
            | RemoteCall::Open { .. }
            | RemoteCall::WorksOn { .. }
            | RemoteCall::Stop { .. }
            | RemoteCall::Manage { .. }
            | RemoteCall::Control { .. }
            | RemoteCall::Offers { .. }
            | RemoteCall::Upload { .. } => Purpose::Agents,
            RemoteCall::Agents
            | RemoteCall::Look { .. }
            | RemoteCall::Find { .. }
            | RemoteCall::Who { .. }
            | RemoteCall::Since { .. }
            | RemoteCall::Facts
            | RemoteCall::Interfaces
            | RemoteCall::Relay { .. } => Purpose::Look,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum RemoteReply {
    ComposerUploaded(Box<crate::composer::UploadResult>),
    Composer(Box<crate::composer::Updated>),
    PageAction {
        result: crate::pages::PageResult,
    },
    FilesFound(crate::file_search::FileSearchReply),
    Thumbnail(crate::thumbnail::Thumbnail),
    Lines(crate::lines::Reply),
    ClaudeConsent {
        accepted: bool,
        changed: bool,
    },
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
    Offers(Box<Offers>),
    /// A watch ended: the caller left, or the session's machine stopped it.
    Watched,
    /// A conversation's setting there changed as asked.
    Set,
    Facts(MachineFacts),
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
