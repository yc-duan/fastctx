//! The frames a machine and the hub exchange.
//!
//! A machine reaches the hub over TLS, by its own line or through a machine
//! that allows direct connections and forwards the hub's bytes as they are,
//! and opens a WebSocket on one of two paths: the control connection, which
//! each member holds open for as long as its node runs, and a data
//! connection, opened for one transfer of bytes too large for a control
//! message. Every frame on either is one
//! JSON value in a binary WebSocket message; after a data connection is
//! paired, the bytes it carries follow as raw binary messages.
//!
//! A control connection opens with what it is for: a member's session,
//! signed for with the member's key; a password login, which runs OPAQUE
//! over the connection (`crate::login`); a login with an enrollment token
//! (`crate::token`); opening an account on a hub that lets anyone; or one
//! end of a data connection.

use fastctx_types::access::TownId;
use fastctx_types::town::{AfootJob, AfootResident, ResidentNow, RunningJob};
use fastctx_types::{AccountName, HubName, MachineName, TownName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::directory::{Kind, Paths, View};
use crate::envelope::Envelope;
use crate::ids::HubId;
use crate::sign::{Bytes, Digest, PublicKey, Signature, Signed};

/// Raised whenever a change would make one side misread the other.
pub const PROTOCOL: u32 = 12;

/// Protocol 12's transport is also understood by released protocol 11 hubs.
/// Feature-bearing calls are selected from the destination node's facts.
pub const GREETING_PROTOCOL: u32 = 11;

pub fn accepts(protocol: u32) -> bool {
    matches!(protocol, GREETING_PROTOCOL | PROTOCOL)
}

pub const CONTROL_PATH: &str = "/fastctx/world/4";
pub const DATA_PATH: &str = "/fastctx/world/4/data";

/// The largest frame on a control connection. Heartbeats, calls and
/// letters share that connection, so nothing on it may be large enough to
/// hold the others up; anything larger goes over a data connection.
pub const MAX_CONTROL_FRAME: usize = 256 * 1024;

/// The largest binary message on a data connection.
pub const MAX_DATA_MESSAGE: usize = 1024 * 1024;

/// How long either side waits without hearing from the other before it
/// takes the connection for dead: three heartbeats (`world-network.md`).
pub const SILENCE: std::time::Duration = std::time::Duration::from_secs(30);

/// How often a machine sends a heartbeat.
pub const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(10);

/// The label of the TLS exporter a machine signs to prove which connection
/// it is on (RFC 9266), and which a login binds itself to.
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-Channel-Binding";

/// A frame from a machine to the hub.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "f")]
pub enum Up {
    /// The first frame on every connection.
    Hello { protocol: u32, version: String, intent: Intent },
    Ping {
        n: u64,
        /// How this machine's lines go, when that changed since it last
        /// said; older hubs ignore it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paths: Option<Paths>,
        /// How many bytes of this connection this machine took from the hub
        /// (`crate::pulse`); an earlier release says none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        took: Option<u64>,
        /// The last of its own letters this machine knows the hub holds, why
        /// the hub refused any of them already recorded (`Down::Ack`); an
        /// earlier release says none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held: Option<u64>,
    },
    /// The second half of a password login (`Intent::Login`): the last
    /// OPAQUE message, and the computer or phone that logs in.
    LoginFinish { finalization: Bytes, member: Joining },
    /// The second half of opening an account (`Intent::Register`).
    RegisterFinish { upload: Bytes },
    /// A letter for the hub to keep until its receiver has it. `seq` counts
    /// this machine's letters; the hub acknowledges each once it holds it.
    Send { seq: u64, message: Envelope },
    /// A call for another member, answered once with `Answered`.
    Call { id: u64, message: Envelope },
    /// The answer to the call the hub numbered `call`.
    Answer { call: u64, message: Envelope },
    /// The caller no longer waits for its call `id`; an answer that comes
    /// for it is dropped. What the call started runs on.
    Cancel { id: u64 },
    /// This machine holds the letter the hub numbered `seq`, and all
    /// before it.
    Ack { seq: u64 },
    /// A question for the hub itself.
    Ask { id: u64, question: Question },
    /// A ledger question in ordered JSON parts, only for hubs advertising ledger_parts.
    QuestionPart { id: u64, index: u32, total: u32, text: String },
    /// Abandons only an incomplete question; an accepted append still finishes.
    CancelQuestion { id: u64 },
    /// How what is afoot on this machine changed since it last told this
    /// session (`world-network.md`, 近况); sent only to a hub whose view
    /// says it takes it.
    Afoot { change: Box<fastctx_types::town::AfootChange<AfootResident, AfootJob>> },
    /// What this member did with the deletion the hub numbered `id`
    /// (`Down::Forget`).
    Forgot { id: u64, outcome: Forgot },
    /// The versions this machine runs and knows of; sent only to a hub
    /// whose view says it takes them, when the session opens and whenever
    /// they change.
    Versions { report: Box<fastctx_types::update::Report> },
    /// This member could not fetch the package of the hub's request `id`
    /// (`Down::Fetch`), and why.
    Unfetched { id: u64, why: String },
    /// Progress of a fetch that explicitly requested it.
    FetchProgress { id: u64, progress: fastctx_types::update::PackageProgress },
    /// The request, including taking its bytes, is finished or abandoned.
    PackageDone { id: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Intent {
    /// A member's control connection. `proof` is its key's signature of the
    /// TLS exporter, so the signature proves nothing on any other connection.
    /// `through` names the machine that forwards it to the hub, if any: the
    /// hub sees only that machine's address.
    Session {
        key: PublicKey,
        proof: Signature,
        letters: Letters,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        through: Option<String>,
        /// Whether the machine can take direct connections and forward
        /// others' to the hub (`crate::peer`); older nodes do not say.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        direct: bool,
        /// The addresses of its own network cards, which its direct
        /// address may be chosen from.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        interfaces: Vec<std::net::IpAddr>,
        /// The address this connection was dialed to, as the machine dialed
        /// it: a hub that moved tells by it which machines still come
        /// through the old place (`crate::moving`). Older machines omit it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dialed: Option<std::net::SocketAddr>,
        /// The machine reads machine and hub names of any language
        /// (`world-network.md`, 机器的名字). Earlier releases do not say so,
        /// and cannot read such a name in anything the hub sends them.
        #[serde(default)]
        any_names: bool,
        /// The machine takes a new name the hub gives it, and reads the
        /// records of renames. Earlier releases do not say so: one renamed
        /// would never connect again under a name it did not ask for.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        renames: bool,
        /// The member reads `Down::Afoot`. Earlier releases do not say so,
        /// and would end the session on a frame they cannot read.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        afoot: bool,
        /// The member deletes what it keeps of a conversation when told
        /// (`Down::Forget`). Earlier releases do not say so, and would end
        /// the session on a frame they cannot read.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        forgets: bool,
        /// The member reads visitors among the accounts it is sent
        /// (`crate::directory::Level::Visitor`). Earlier releases do not say
        /// so, and could not read the view that names one: they are sent
        /// visitors as members.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        roles: bool,
        /// The member reads `Down::Versions` and `Down::Fetch`. Earlier
        /// releases do not say so, and would end the session on a frame
        /// they cannot read.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        versions: bool,
        /// Reads a version snapshot in bounded parts, published only when complete.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        version_parts: bool,
        /// The member reads the records of updates kept for machines away
        /// and of updates made as another asked. Earlier releases do not say
        /// so, and could read nothing of a machine log page with one.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        due: bool,
        /// The member reads the records of projects deleted. Earlier
        /// releases do not say so, and could read nothing of a machine log
        /// page with one.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        project_deletes: bool,
        /// Accepts a running job's project selection at its resident's home.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        project_entry: bool,
        /// Understands shared attachment lifetimes and account-scoped project deletion.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        conversation_deletions: bool,
        /// The member reads the records of profiles written. Earlier
        /// releases do not say so, and could read nothing of a machine log
        /// page with one.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        profiles: bool,
        /// Reads account record invalidations independently of the directory.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        account_records: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        record_keys: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        skill_files: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        conversation_plans: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        scratch_locations: bool,
        /// A machine taken off the hub that was to uninstall Fastctx then,
        /// and could not do all of it: what was left, and why. The hub
        /// keeps it for its machine log, and refuses the session as before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        left: Option<String>,
        /// The member reads `Down::Cancelled`. Earlier releases do not say
        /// so, and end their session at a frame they cannot read.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        cancels: bool,
        /// The member reads `Down::Presence`: who is online, as it changes,
        /// in place of looking at the whole directory again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        presence: bool,
        /// The member reads `Down::Part`: an answer too large for one frame,
        /// in parts. One that does not is told such an answer is too large.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        parts: bool,
        /// The receiver verifies a large call's control-frame authorization facts against its body.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        call_details: bool,
        /// The member reads `Down::View`: what changed in its view, in
        /// place of being told to ask for the whole view again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        view_deltas: bool,
    },
    /// A password login to `account`: the first OPAQUE message. The hub
    /// answers `Down::LoginResponse`; the device finishes with
    /// `Up::LoginFinish`.
    Login { account: AccountName, request: Bytes },
    /// A login with an enrollment token: `token` is the token's public key,
    /// `proof` its signature of the TLS exporter.
    Token { token: PublicKey, proof: Signature, member: Joining },
    /// What the token `token` lets a device into, without using it: the
    /// hub answers `Down::TokenInfo` and closes. Sent only to a hub that
    /// says it registers managed machines; an earlier hub refuses the
    /// connection, which the asking device takes as that. `member` is the
    /// asking device's key, so one that came in by it already is told what
    /// using it again would tell it; an earlier hub reads past it.
    Peek {
        token: PublicKey,
        proof: Signature,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        member: Option<PublicKey>,
    },
    /// Opening `account` on a hub that lets anyone: the first OPAQUE
    /// registration message. The hub answers `Down::RegisterResponse`.
    Register { account: AccountName, request: Bytes },
    /// One end of the data connection `stream`.
    Data { key: PublicKey, proof: Signature, stream: u64, end: End },
    /// A package the hub holds, from the byte `from` on: the hub says the
    /// whole package's size, `{"size":…}`, then sends the bytes, and the
    /// member says it has them all. Sent only to a hub whose view says it
    /// keeps versions.
    Package { key: PublicKey, proof: Signature, version: String, target: String, from: u64 },
    /// The package of the hub's request `id` (`Down::Fetch`): the member
    /// says its size, `{"size":…}`, then sends the bytes, and the hub says
    /// `"done"` once it has checked them, or why it did not take them.
    Deliver { key: PublicKey, proof: Signature, id: u64 },
    /// Whether the hub takes sessions now, through this address: answered
    /// `Down::Ready`, and closed. A machine on a later address of its list
    /// asks the earlier ones.
    Probe,
    /// The old hub of the move `id`, dialing the new place for `role`;
    /// `proof` is the move's key's signature of the TLS exporter
    /// (`crate::moving`).
    Handover { id: String, role: crate::moving::HandoverRole, proof: Signature },
}

/// A computer or phone logging in: the name it asks for, its key, and
/// whether it registers as a machine of the account too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Joining {
    pub name: MachineName,
    pub key: PublicKey,
    /// The key's signature of the TLS exporter.
    pub proof: Signature,
    pub kind: Kind,
    pub register: bool,
    /// As on a session: the device reads names of any language.
    #[serde(default)]
    pub any_names: bool,
}

/// Where a machine's letters stand, as it remembers: the hub's letters to it
/// it has taken, and which numbering of its own letters it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Letters {
    /// Chosen when the machine's letter records are made; a machine that
    /// lost them starts a new numbering, and the hub forgets the old one.
    pub epoch: u64,
    /// The hub's numbering of its letters to this machine, and the last of
    /// them this machine took. `None` before the first welcome.
    pub taken: Option<(u64, u64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    /// The machine the bytes come from.
    Source,
    /// The machine the bytes go to.
    Sink,
}

/// A data connection that goes with a call or an answer: its number,
/// chosen at random by whoever makes it, and its two ends. The hub pairs
/// the two ends only for a ticket a call or answer it passed on named, and
/// only while what that call was allowed still holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub stream: u64,
    pub source: MachineName,
    pub sink: MachineName,
}

/// What a transfer carries, said in a call or an answer before the bytes
/// flow, so the sink can check every byte it takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carried {
    pub stream: u64,
    pub bytes: u64,
    pub sha256: Digest,
}

/// A frame from the hub to a machine.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "f")]
pub enum Down {
    /// The session is open. `taken` is the last of this machine's letters
    /// the hub holds in the numbering `epoch`; `letters` is the hub's
    /// numbering of its letters to this machine.
    Welcome {
        hub: HubId,
        hub_name: HubName,
        name: MachineName,
        version: String,
        taken: u64,
        letters: u64,
        /// This hub separately acknowledges completion of its initial status snapshot.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        afoot_pending: Option<bool>,
    },
    /// The hub's half of a password login.
    LoginResponse {
        response: Bytes,
    },
    /// What a token lets a device into, asked before it is used
    /// (`Intent::Peek`): whose, and whether as a managed machine.
    TokenInfo {
        hub: HubId,
        hub_name: HubName,
        account: AccountName,
        managed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        expires: Timestamp,
    },
    /// The hub's half of opening an account, or of setting a password.
    RegisterResponse {
        response: Bytes,
    },
    /// The login was taken: this computer or phone is the member `name` of
    /// the hub, logged in to `account`.
    LoggedIn {
        hub: HubId,
        hub_name: HubName,
        name: MachineName,
        account: AccountName,
    },
    /// The account is open; log in to it.
    Registered,
    /// The data connection is paired; bytes follow.
    Paired,
    /// The connection is refused or ended, and why.
    Refused {
        reason: Refusal,
    },
    Pong {
        n: u64,
        /// The hub's time as it answered, by its clock (`crate::link`'s
        /// clock); an older hub says none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<jiff::Timestamp>,
        /// How many bytes of this connection the hub took from the machine
        /// (`crate::pulse`); an older hub says none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        took: Option<u64>,
        /// All initial status frames preceded this acknowledgement on the wire.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        afoot_ready: bool,
    },
    /// The hub holds this machine's letter `seq` and all before it; when
    /// the hub cannot deliver it, `refused` says why and the letter is gone.
    Ack {
        seq: u64,
        refused: Option<String>,
    },
    /// A letter for this machine, numbered `seq` in the hub's numbering.
    Deliver {
        seq: u64,
        message: Envelope,
    },
    /// A call for this machine, numbered `call` by the hub.
    Incoming {
        call: u64,
        message: Envelope,
    },
    /// How the call `id` this machine made is going.
    Status {
        id: u64,
        status: CallStatus,
    },
    /// The answer to the call `id` this machine made.
    Answered {
        id: u64,
        message: Envelope,
    },
    /// The answer to the question `id`.
    Told {
        id: u64,
        answer: Result<Told, String>,
    },
    /// The directory changed in what this member sees of it.
    DirectoryChanged,
    /// The hub takes sessions now (`Intent::Probe`).
    Ready,
    /// A project this member may use has new entries in its ledger.
    LedgerChanged {
        town: TownId,
    },
    AccountRecordChanged {
        changed: fastctx_types::account_records::Changed,
    },
    /// How what is afoot on `machine` changed as this member may see it,
    /// since this session was last told of it; and, to the machine's owner
    /// alone, how many agents are at work or waiting there now.
    Afoot {
        machine: MachineName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        busy: Option<u32>,
        change: Box<fastctx_types::town::AfootChange<ResidentNow, RunningJob>>,
    },
    /// Delete what this member keeps of a conversation, and say how that
    /// went (`Up::Forgot`). Sent only to a member whose session says it
    /// forgets: now, and at each session it opens until it says it is done.
    Forget {
        forgetting: Forgetting,
    },
    /// The versions of the network as the hub knows them, sent when the
    /// session opens and whenever they change, only to a member whose
    /// session says it reads them.
    Versions {
        network: Box<fastctx_types::update::Network>,
    },
    /// A version snapshot, starting at index zero. A new zero supersedes an
    /// incomplete snapshot; no part becomes visible before the last one.
    VersionPart {
        index: u32,
        last: bool,
        network: Box<fastctx_types::update::Network>,
    },
    /// Fetch this package for the network, and deliver it
    /// (`Intent::Deliver`) or say why not (`Up::Unfetched`). Sent only to a
    /// member that reads versions and said it fetches for others, or holds
    /// the package.
    Fetch {
        fetch: Box<fastctx_types::update::Fetch>,
    },
    /// Sent only to a helper that advertised package progress.
    FetchCancelled {
        id: u64,
    },
    /// Sent only for a package question that requested progress.
    PackageProgress {
        id: u64,
        progress: fastctx_types::update::PackageProgress,
    },
    /// The call `call` the hub passed on (`Incoming`) is over for its
    /// caller, who gave it up, left the hub, or lost the right it was made
    /// under: one that only reads stops (`fastctx_types::contract`). Sent
    /// only to a member whose session says it reads this.
    Cancelled {
        call: u64,
    },
    /// How `machine` is connected now, as the member may see it, the
    /// hub's `rev`th change in who is online (`crate::directory::View::presence`).
    /// Sent only to a member whose session says it reads this.
    Presence {
        machine: MachineName,
        presence: crate::directory::Presence,
        rev: u64,
    },
    /// The part `index` of `total` of the answer to the question `id`, as
    /// JSON text, when the whole is too large for one frame: put together,
    /// they are what `Told` would carry. Sent only to a member whose session
    /// says it reads this.
    Part {
        id: u64,
        index: u32,
        total: u32,
        text: String,
    },
    /// What changed in this member's view (`crate::view_delta`). Sent only
    /// to a member whose session says it reads this; one too large for a
    /// frame goes as `DirectoryChanged`.
    View {
        delta: Box<crate::view_delta::ViewDelta>,
    },
}

/// The most bytes of text one `Down::Part` carries.
pub const PART: usize = 192 * 1024;

/// The most bytes an answer in parts may take in all.
pub const PARTS_AT_MOST: usize = 16 * 1024 * 1024;

/// A conversation the hub has its members delete what they keep of
/// (`world-colleagues.md`, 用户在任何地方和居民说话).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forgetting {
    /// The hub's number of the deletion, which what a member says of it
    /// names.
    pub id: u64,
    pub conversation: fastctx_types::Address,
    /// The account it is deleted for: one it worked for.
    pub account: AccountName,
}

/// What a member did with a deletion (`Down::Forget`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Forgot {
    /// Nothing of the conversation is kept here any more.
    Done,
    /// The machine it lives on says it does not work for that account:
    /// nothing of it is deleted anywhere.
    Refused { reason: String },
    /// It could not be deleted here now; the hub asks again when this member
    /// next connects.
    Failed { reason: String },
}

/// Why the hub refuses or ends a connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Refusal {
    #[error("the hub speaks protocol {hub}, this fastctx speaks {PROTOCOL}; the hub is fastctx {version}")]
    Protocol { hub: u32, version: String },
    #[error("this computer is not logged in to the hub")]
    NotMember,
    #[error("{}", removed_said(by.as_ref(), at, reason, *dissolved))]
    Removed {
        by: Option<AccountName>,
        at: Timestamp,
        reason: String,
        /// The whole network was dissolved, this computer's place with it;
        /// an earlier release reads past it, and takes it as removed.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dissolved: bool,
        /// What it is to do now: uninstall Fastctx, as its remover asked.
        /// An earlier release reads past it, and only leaves.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        then: Option<fastctx_types::uninstall::Then>,
    },
    #[error("{reason}")]
    Login { reason: String },
    #[error("another connection of this machine replaced this one")]
    Replaced,
    /// Sent only to a machine whose session said it takes a new name.
    #[error("this machine is called {to} on the hub now")]
    Renamed { to: MachineName },
    #[error("the hub is shutting down")]
    ShuttingDown,
    #[error("{reason}")]
    Broken { reason: String },
}

/// How a removal is said: by whom and when, and why; a dissolution as the
/// end of the whole network.
fn removed_said(by: Option<&AccountName>, at: &Timestamp, reason: &str, dissolved: bool) -> String {
    let when = at.strftime("%Y-%m-%d %H:%M:%S UTC");
    let by = by.map(|by| format!(" by the account {by}")).unwrap_or_default();
    if dissolved {
        return format!("the network was dissolved{by} at {when}");
    }
    let why = if reason.is_empty() { String::new() } else { format!(": {reason}") };
    format!("this computer was removed from the hub{by} at {when}{why}")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CallStatus {
    /// The hub handed the call to the machine it is for.
    Delivered,
    /// That machine is not connected; it was last connected at `last_seen`.
    Offline { last_seen: Option<Timestamp> },
    /// No member has that name.
    NotMember,
    /// The hub did not pass the call on, and why: nothing of it ran.
    Refused { reason: String },
    /// That machine's connection ended before it answered; whether it did
    /// what was asked is not known.
    Lost,
    /// The right the call was made under was taken back while it ran; the
    /// hub stopped passing on what belongs to it. Whether it did what was
    /// asked is not known.
    Cut { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Question {
    RecordWatching {
        account: AccountName,
        record_kind: String,
        watch: String,
        following: bool,
    },
    RecordFollowing {
        account: AccountName,
        record_kind: String,
        key: String,
        following: bool,
    },
    SkillFiles {
        account: AccountName,
        request: fastctx_types::skill_files::Request,
    },
    PlanCopies {
        account: AccountName,
        request: fastctx_types::plan_copies::Request,
    },
    AccountRecords {
        account: AccountName,
        request: fastctx_types::account_records::Request,
    },
    NetworkUpdates {
        account: AccountName,
        request: fastctx_types::update::ViewCall,
    },
    /// Machine-layer history visible to this member in `account`, newest
    /// first and strictly before the cursor when supplied.
    MachineLog {
        account: AccountName,
        before: Option<u64>,
    },
    /// Linux archives and the hub's actual TLS certificate. Without opt-in,
    /// both architecture digests must be present, as older callers require.
    KernelRelease {
        version: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_partial: bool,
        /// Which of the archives a hub cannot pack itself the line being
        /// made downloads, brought in when the hub lacks them; an earlier
        /// caller, which never says, gets the Windows one brought.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        wanted: Vec<String>,
    },
    /// This member's view of the directory.
    Directory,
    /// A change made as `account`, which must be logged in on the asking
    /// member; changes to other accounts and to the hub need it to be the
    /// super administrator's.
    Manage {
        account: AccountName,
        change: Change,
    },
    /// Metadata only, including for an account whose agent this member hosts.
    ProjectIndex {
        town: TownId,
        account: AccountName,
    },
    /// Signed document bytes, only on a device logged into the account.
    MemoryRead {
        town: TownId,
        account: AccountName,
        name: String,
        version: Option<u32>,
    },
    /// Append project memory explicitly for one account.
    MemoryWrite {
        town: TownId,
        account: AccountName,
        entry: Signed,
    },
    /// Keep this entry in the ledger of `town`.
    LedgerWrite {
        town: TownId,
        entry: Signed,
    },
    /// The legacy ledger catalog. Current hubs list projects available to
    /// this member's accounts; older hubs limited it to work locations.
    LedgerTowns,
    /// Every project this member may use, each with the number of its
    /// latest entry. This does not subscribe the asker to replication.
    LedgerCatalog,
    /// This member finished the project deletion delivered in its view.
    ProjectDeleted {
        town: TownId,
    },
    /// A project's entries after the one numbered `after`, as many as fit
    /// in one frame.
    LedgerRead {
        town: TownId,
        after: u64,
    },
    /// Moving the hub, asked as `account`, which must be the super
    /// administrator's and logged in on the asking member.
    HubMove {
        account: AccountName,
        request: fastctx_types::hub_move::HubMoveRequest,
    },
    /// What the machine the hub moves to asks of it.
    HubArrival {
        request: crate::moving::ArrivalRequest,
    },
    /// This member called `to` directly, for `account`, to work for the
    /// resident `resident`: what the hub notes of a call it passes on, for
    /// the letters that tell of that work later (`world-network.md`, 寄信).
    Worked {
        to: MachineName,
        account: AccountName,
        purpose: fastctx_types::access::Purpose,
        resident: fastctx_types::Address,
    },
    /// This member, being checked for direct connections, listens at the
    /// ports of `at` now, or could not; with what it knows of its own
    /// firewall.
    Listening {
        at: Vec<std::net::SocketAddr>,
        outcome: Result<Firewall, String>,
    },
    /// Delete the conversation at `conversation`, which works for
    /// `account`, logged in on the asking member, from every member keeping
    /// something of it: the machine it lives on first, unless that is the
    /// asking member, which deleted what it kept before it asked. Answered
    /// `Told::Done` once the hub holds it. Asked only of a hub whose view
    /// says it forgets.
    Forget {
        account: AccountName,
        conversation: fastctx_types::Address,
    },
    /// The package `target` of the release `version`, for this member to
    /// download (`Intent::Package`): answered `Told::Package` once the hub
    /// holds it, having fetched it first, or asked a machine that can, when
    /// it did not. Asked only of a hub whose view says it keeps versions.
    /// Prepares the packages for an explicitly queued update before it starts.
    PrepareUpdate {
        machine: MachineName,
        version: String,
    },
    Package {
        version: String,
        target: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        progress: bool,
    },
}

/// What a machine knows of its own firewall when it starts taking direct
/// connections, which tells the layer that stopped the hub's check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Firewall {
    /// The World let the port in through this machine's firewall.
    Opened,
    /// A firewall the World does not manage is on.
    Other { name: String },
    /// No firewall the World knows of is on.
    NoneFound,
    /// Whether this machine's firewall lets the port in is not known.
    Unknown { why: String },
}

/// What an account changes on its hub.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum Change {
    Collab {
        collab: fastctx_types::collab::Collab,
    },
    /// Changes only the account's project-memory switch, preserving its other configuration.
    MemorySettings {
        version: u64,
        enabled: bool,
    },
    /// Changes only the account's layer (`world-layers.md`), over the
    /// profile version it was read at. Sent only to a hub whose view says
    /// it keeps layers.
    Layer {
        version: u64,
        layer: fastctx_types::layers::Layer,
    },
    /// Writes the account's profile over the version it was read at; a
    /// machine that works for the account may (`world-user-config.md`).
    Profile {
        profile: fastctx_types::profile::Profile,
    },
    /// Changes only the fourth profile component, over the shared profile version.
    Subagents {
        version: u64,
        subagents: Option<fastctx_types::profile::Subagents>,
    },
    /// Shares `machine`, one of the account's own, with `to`, or no longer.
    Share {
        machine: MachineName,
        to: AccountName,
        shared: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        jobs: Option<u16>,
    },
    /// Lets `agent`'s login `home` on `machine`, one of the account's own,
    /// assist `to`, or no longer; none is that agent's default login there.
    /// Given again, the assistance takes the deadline and the allowances
    /// given now, or none. Sent under the name hubs of every
    /// version read: a hub that cannot read a change ends the connection
    /// that sent it.
    #[serde(rename = "lend")]
    Assist {
        machine: MachineName,
        to: AccountName,
        agent: fastctx_types::AgentName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<std::path::PathBuf>,
        #[serde(rename = "lent")]
        assisting: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<Timestamp>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weekly: Option<fastctx_types::usage::Usd>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        daily: Option<fastctx_types::usage::Usd>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        five_hour: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turns: Option<u16>,
    },
    /// The super administrator sets the plan shown for `agent`'s login
    /// `home` on `machine`, or takes it away.
    Plan {
        machine: MachineName,
        agent: fastctx_types::AgentName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<std::path::PathBuf>,
        plan: Option<fastctx_types::accounts::PlanLabel>,
    },
    /// `machine` no longer acts for the account.
    StopActing {
        machine: MachineName,
    },
    /// Allows direct connections to `machine` at `at`, or, with none, no
    /// longer: the super administrator's change. The hub checks from
    /// outside that it reaches the machine there before it counts; an
    /// address only machines on the same network reach it cannot check.
    Direct {
        machine: MachineName,
        at: Vec<std::net::SocketAddr>,
    },
    /// Takes the member `name` out of the account: a device's login of it,
    /// or one of its machines. The super administrator takes it off the
    /// hub.
    Remove {
        name: MachineName,
        reason: String,
        /// What the machine does once it hears it was taken off: only for
        /// a machine its asker may uninstall. Sent only to a hub whose view
        /// says it uninstalls.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        then: Option<fastctx_types::uninstall::Then>,
    },
    /// Renames `machine`, one of the account's own or, for the super
    /// administrator, any, to `to`; its names before keep naming it. Sent
    /// only to a hub whose view says it renames.
    Rename {
        machine: MachineName,
        to: MachineName,
    },
    /// Puts `machine`, one of the account's own or, for the super
    /// administrator, a member's, in the group `group`, or in none. Sent
    /// only to a hub whose view says it keeps groups.
    Group {
        machine: MachineName,
        group: Option<String>,
    },
    /// Renames the group `from` of `owner`'s machines to `to`, or ends it.
    RenameGroup {
        owner: AccountName,
        from: String,
        to: Option<String>,
    },
    /// The asking member logs out of the account.
    Logout,
    /// An enrollment token for the account (`crate::token`), valid until
    /// `expires`; `register` makes the computer that uses it a machine of
    /// the account, and `managed` a managed machine of it
    /// (`crate::directory::Member::managed`). `uses` is how many computers
    /// may use it, one when none, any number at zero; `label` names it.
    /// Sent with any of the three only to a hub whose view says it
    /// registers managed machines: an earlier hub would make a one-time
    /// token for a full login.
    Token {
        register: bool,
        name: Option<MachineName>,
        kind: Kind,
        expires: Timestamp,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        managed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uses: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// How it is given (`fastctx_types::accounts::TokenWay`), kept to be
        /// told; an earlier hub reads past it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        way: Option<fastctx_types::accounts::TokenWay>,
    },
    /// No device comes in by the token whose key is `token` any more: the
    /// account that made it, or the super administrator, takes it back.
    RevokeToken {
        token: PublicKey,
    },
    /// The super administrator makes `name` a visitor at once, or a member
    /// once the account itself agrees.
    SetRole {
        name: AccountName,
        visitor: bool,
    },
    /// The asking account agrees to become a member, or does not.
    AcceptRole {
        accept: bool,
    },
    /// Sets the password of `target`, the account itself or, for the super
    /// administrator, another: the first OPAQUE registration message.
    PasswordStart {
        target: AccountName,
        request: Bytes,
    },
    /// The last OPAQUE registration message for `target`.
    PasswordFinish {
        target: AccountName,
        upload: Bytes,
    },
    /// Opens an account, a visitor's when `visitor`; its password is set
    /// next. Sent with `visitor` only to a hub whose view has roles.
    OpenAccount {
        name: AccountName,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        visitor: bool,
    },
    SetAccount {
        name: AccountName,
        locked: bool,
    },
    /// Hands being the super administrator to `to`; the asking account
    /// becomes a member.
    Transfer {
        to: AccountName,
    },
    /// Deletes an account: its machines leave the hub, what it shared and
    /// was shared and the assistance it gave and was given go, its projects
    /// are archived for the super administrator to hand on.
    DeleteAccount {
        name: AccountName,
    },
    /// Whether anyone who reaches the hub may open an account.
    Settings {
        self_registration: bool,
    },
    /// The version the network asks each machine to run at least, and
    /// whether its managed machines update by themselves. Sent only to a hub
    /// whose view says it keeps versions.
    Updates {
        policy: fastctx_types::update::Policy,
    },
    /// How fast the hub takes packages in at most, in bytes a second; none,
    /// its default (`fastctx_types::update::INTAKE`). The super
    /// administrator's; sent only to a hub whose view says it paces intake.
    Intake {
        rate: Option<u64>,
    },
    /// `machine`, one the account may update and away now, updates to
    /// `version` once it is back; none takes back what was kept. Sent only
    /// to a hub whose view says it keeps updates due.
    UpdateDue {
        machine: MachineName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// Reserves the package for a batch without telling this machine to start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch: Option<String>,
    },
    /// The super administrator dissolves the network named `name`, typed
    /// as it is: every machine and device is taken off it, and the hub lets
    /// nobody in again. Sent only to a hub whose view says it dissolves.
    Dissolve {
        name: HubName,
    },
    /// Opens a project in the account.
    OpenProject {
        town: TownId,
        name: TownName,
    },
    /// Lets the account `with` read and write the project's records, or no
    /// longer.
    ShareProject {
        town: TownId,
        with: AccountName,
        shared: bool,
    },
    /// Hands a project to another account: its owner's change, and the
    /// super administrator's for a project archived with a deleted account.
    MoveProject {
        town: TownId,
        to: AccountName,
    },
    /// Deletes a project and its ledger: its owner's change. Sent only to a
    /// hub whose view says it deletes projects.
    DeleteProject {
        town: TownId,
    },
    /// Lets `machine` relay its owner's other machines to the hub at `at`,
    /// or, with none, no longer: its owner's change, a visitor's too. Sent
    /// only to a hub whose view says it keeps relays.
    Relay {
        machine: MachineName,
        at: Vec<std::net::SocketAddr>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Told {
    SkillFiles {
        reply: fastctx_types::skill_files::Reply,
    },
    PlanCopies {
        reply: fastctx_types::plan_copies::Reply,
    },
    AccountRecords {
        response: fastctx_types::account_records::Response,
    },
    ProjectIndex {
        index: Box<fastctx_types::memory::ProjectIndex>,
    },
    MemoryDocument {
        entry: Signed,
        versions: Vec<fastctx_types::memory::Revision>,
        evidence: Vec<fastctx_types::memory::Evidence>,
    },
    NetworkUpdates {
        reply: fastctx_types::update::ViewReply,
    },
    MachineLog {
        page: fastctx_types::machine_log::Page,
    },
    CollabWritten {
        version: u64,
    },
    ProfileWritten {
        version: u64,
    },
    KernelRelease {
        release: fastctx_types::kernel::KernelRelease,
        certificate: Vec<u8>,
    },
    Directory {
        view: Box<View>,
    },
    Done,
    /// The hub holds the package asked for, of this many bytes.
    Package {
        size: u64,
    },
    /// The token, as text to carry to the device.
    Token {
        text: String,
    },
    /// The hub's half of setting a password.
    RegisterResponse {
        response: Bytes,
    },
    /// The entry is the project's entry `seq`.
    Written {
        seq: u64,
    },
    Towns {
        towns: Vec<(TownId, u64)>,
    },
    /// Entries in order, each with its number; `more` when others follow.
    Entries {
        entries: Vec<(u64, Signed)>,
        more: bool,
    },
    HubMove {
        reply: fastctx_types::hub_move::HubMoveReply,
    },
    /// The hub's identity, for the machine it moves to.
    HubIdentity {
        identity: Box<crate::moving::Identity>,
    },
}

/// Encodes a frame for the wire.
pub fn encode(frame: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(frame).expect("frames always serialize")
}
