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
        /// The machine reads machine and hub names of any language
        /// (`world-network.md`, 机器的名字). Earlier releases do not say so,
        /// and cannot read such a name in anything the hub sends them.
        #[serde(default)]
        any_names: bool,
    },
    /// A password login to `account`: the first OPAQUE message. The hub
    /// answers `Down::LoginResponse`; the device finishes with
    /// `Up::LoginFinish`.
    Login { account: AccountName, request: Bytes },
    /// A login with an enrollment token: `token` is the token's public key,
    /// `proof` its signature of the TLS exporter.
    Token { token: PublicKey, proof: Signature, member: Joining },
    /// Opening `account` on a hub that lets anyone: the first OPAQUE
    /// registration message. The hub answers `Down::RegisterResponse`.
    Register { account: AccountName, request: Bytes },
    /// One end of the data connection `stream`.
    Data { key: PublicKey, proof: Signature, stream: u64, end: End },
    /// Whether the hub takes sessions now, through this address: answered
    /// `Down::Ready`, and closed. A machine on a later address of its list
    /// asks the earlier ones.
    Probe,
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
    },
    /// The hub's half of a password login.
    LoginResponse {
        response: Bytes,
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
}

/// Why the hub refuses or ends a connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Refusal {
    #[error("the hub speaks protocol {hub}, this fastctx speaks {PROTOCOL}; the hub is fastctx {version}")]
    Protocol { hub: u32, version: String },
    #[error("this computer is not logged in to the hub")]
    NotMember,
    #[error("this computer was removed from the hub{} at {}{}", by.as_ref().map(|by| format!(" by the account {by}")).unwrap_or_default(), .at.strftime("%Y-%m-%d %H:%M:%S UTC"), if reason.is_empty() { String::new() } else { format!(": {reason}") })]
    Removed { by: Option<AccountName>, at: Timestamp, reason: String },
    #[error("{reason}")]
    Login { reason: String },
    #[error("another connection of this machine replaced this one")]
    Replaced,
    #[error("the hub is shutting down")]
    ShuttingDown,
    #[error("{reason}")]
    Broken { reason: String },
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
    /// Machine-layer history visible to this member in `account`, newest
    /// first and strictly before the cursor when supplied.
    MachineLog { account: AccountName, before: Option<u64> },
    /// Linux archives and the hub's actual TLS certificate. Without opt-in,
    /// both architecture digests must be present, as older callers require.
    KernelRelease {
        version: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_partial: bool,
    },
    /// This member's view of the directory.
    Directory,
    /// A change made as `account`, which must be logged in on the asking
    /// member; changes to other accounts and to the hub need it to be the
    /// super administrator's.
    Manage { account: AccountName, change: Change },
    /// Keep this entry in the ledger of `town`.
    LedgerWrite { town: TownId, entry: Signed },
    /// The projects this member spans, each with the number of its latest
    /// entry.
    LedgerTowns,
    /// Every project this member may use, each with the number of its
    /// latest entry. This does not subscribe the asker to replication.
    LedgerCatalog,
    /// A project's entries after the one numbered `after`, as many as fit
    /// in one frame.
    LedgerRead { town: TownId, after: u64 },
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
    Listening { at: Vec<std::net::SocketAddr>, outcome: Result<Firewall, String> },
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
    /// Shares `machine`, one of the account's own, with `to`, or no longer.
    Share {
        machine: MachineName,
        to: AccountName,
        shared: bool,
    },
    /// Lends `agent`'s login `home` on `machine`, one of the account's own,
    /// to `to`, or no longer; none is that agent's default login there.
    /// Lent again, the loan takes the deadline given now, or none.
    Lend {
        machine: MachineName,
        to: AccountName,
        agent: fastctx_types::AgentName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<std::path::PathBuf>,
        lent: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<Timestamp>,
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
    },
    /// The asking member logs out of the account.
    Logout,
    /// A one-time enrollment token for the account (`crate::token`),
    /// valid until `expires`; `register` makes the computer that uses it a
    /// machine of the account.
    Token {
        register: bool,
        name: Option<MachineName>,
        kind: Kind,
        expires: Timestamp,
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
    /// Opens a member's account; its password is set next.
    OpenAccount {
        name: AccountName,
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
    /// lent and was shared and lent goes, its projects are archived for the
    /// super administrator to hand on.
    DeleteAccount {
        name: AccountName,
    },
    /// Whether anyone who reaches the hub may open an account.
    Settings {
        self_registration: bool,
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
    /// Hands a project to another account; the super administrator's change.
    MoveProject {
        town: TownId,
        to: AccountName,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Told {
    MachineLog {
        page: fastctx_types::machine_log::Page,
    },
    CollabWritten {
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
}

/// Encodes a frame for the wire.
pub fn encode(frame: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(frame).expect("frames always serialize")
}
