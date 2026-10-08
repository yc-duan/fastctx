//! What one machine asks another to do, and what it sends a resident who
//! lives there (`world-network.md`, 四种话).

use crate::agents::AgentsReply;
use crate::management::{Managed, Management};
use serde::{Deserialize, Serialize};

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
    /// Run a script there for a resident of the calling machine.
    Run {
        resident: ResidentId,
        town: Option<TownName>,
        request: RunRequest,
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
    },
    /// Go on handing over an `on` job's output from these byte offsets, on
    /// the data connection, after the one it started on broke.
    Follow {
        origin: crate::on::OnOrigin,
        job: JobId,
        out_from: u64,
        err_from: u64,
    },
    /// Stop an `on` job: its sender stopped it on purpose.
    StopOn {
        origin: crate::on::OnOrigin,
        job: JobId,
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
    FindFiles {
        request: crate::file_search::FileSearchRequest,
    },
    /// The user's word on Claude Code's bypass-permissions consent there.
    ClaudeConsent {
        accept: bool,
    },
    /// The documents of a canon there, for a town's map.
    Canon {
        path: String,
    },
    /// Start a colleague there for a resident of the calling machine.
    Ask {
        asker: ResidentId,
        town: Option<TownName>,
        label: String,
        prompt: String,
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
    /// Relay for the machines whose routes lead through this machine, as
    /// `relay` says, or stop relaying. Only a member with management rights
    /// asks this.
    SetRelay {
        relay: Option<crate::world::RelaySetting>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum RemoteReply {
    ComposerUploaded(Box<crate::composer::UploadResult>),
    /// Where the machine relays now; nowhere when it stopped.
    Relaying {
        relays_at: Vec<String>,
    },
    Composer(Box<crate::composer::Updated>),
    PageAction {
        result: crate::pages::PageResult,
    },
    FilesFound(crate::file_search::FileSearchReply),
    ClaudeConsent {
        accepted: bool,
        changed: bool,
    },
    Uploaded(crate::attachments::Attachment),
    Run(RunReply),
    /// An `on` job's end was handed over, or it was stopped.
    OnDone,
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
    Facts(MachineFacts),
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
    /// One durable part of a letter that exceeds the control-frame budget.
    Fragment { id: crate::NoticeId, index: usize, total: usize, text: String },
    /// Something the World tells the resident `to`.
    Notice { to: ResidentId, notice: Notice },
    /// Words from the resident `from` of the sending machine to the resident
    /// `to`, who is owed nothing; `from` is owed the end of the turn they go
    /// into.
    Speak { to: ResidentId, from: ResidentId, text: String },
    /// The resident `resident` of the sending machine left: colleagues that
    /// work for it go to sleep once their turn ends.
    Left { resident: ResidentId },
}
