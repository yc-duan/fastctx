//! The frames a machine and the hub exchange.
//!
//! A machine reaches the hub over TLS and opens a WebSocket on one of two
//! paths: the control connection, which each machine holds open for as long
//! as its node runs, and a data connection, opened for one transfer of
//! bytes too large for a control message. Every frame on either is one
//! JSON value in a binary WebSocket message; after a data connection is
//! paired, the bytes it carries follow as raw binary messages.
//!
//! A frame here only carries what members say; what they say is an
//! envelope signed by the member that says it, around a body sealed for the
//! member it is for (`crate::envelope`), so the hub can route it but not
//! read it, change it or make one up.
//!
//! A relay speaks the hub's side of these frames to the machines below it
//! and the machine's side to its own upstream, on one relay session that
//! carries the sessions of every machine below it (`crate::tree`): each
//! carried session is attached by name, and its frames pass wrapped in
//! `Up::For` and `Down::To`.

use fastctx_types::MachineName;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ledger::TownId;
use crate::sign::{Digest, PublicKey, Secret, Signature, Signed};
use crate::trust::WorldId;

/// Raised whenever a change would make one side misread the other.
pub const PROTOCOL: u32 = 8;

pub const CONTROL_PATH: &str = "/fastctx/world/3";
pub const DATA_PATH: &str = "/fastctx/world/3/data";

/// The largest frame on a control connection. Heartbeats, calls and
/// letters share that connection, so nothing on it may be large enough to
/// hold the others up; anything larger goes over a data connection.
pub const MAX_CONTROL_FRAME: usize = 256 * 1024;

/// The largest frame on a relay session: a control frame wrapped once for
/// the machine it is for (`Down::To`, `Up::For`). Every hop wraps the frame
/// afresh, so one wrapping is the most there is; it adds the variant and
/// field names, a machine name of at most 32 bytes and a tag of at most 20
/// digits, under 100 bytes in all, and a kilobyte leaves room to spare.
pub const MAX_RELAYED_FRAME: usize = MAX_CONTROL_FRAME + 1024;

/// The largest binary message on a data connection.
pub const MAX_DATA_MESSAGE: usize = 1024 * 1024;

/// How long either side waits without hearing from the other before it
/// takes the connection for dead: three heartbeats (`world-network.md`).
pub const SILENCE: std::time::Duration = std::time::Duration::from_secs(30);

/// How often a machine sends a heartbeat.
pub const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(10);

/// The label of the TLS exporter a machine signs to prove which connection
/// it is on (RFC 9266).
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-Channel-Binding";

/// A frame from a machine to the hub.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "f")]
pub enum Up {
    /// The first frame on every connection.
    Hello {
        protocol: u32,
        version: String,
        intent: Intent,
    },
    Ping {
        n: u64,
    },
    /// A letter for the hub to keep until its receiver has it. `seq` counts
    /// this machine's letters; the hub acknowledges each once it holds it.
    Send {
        seq: u64,
        message: Signed,
    },
    /// A call for another member, answered once with `Answered`.
    Call {
        id: u64,
        message: Signed,
    },
    /// The answer to the call the hub numbered `call`.
    Answer {
        call: u64,
        message: Signed,
    },
    /// The caller no longer waits for its call `id`; an answer that comes
    /// for it is dropped. What the call started runs on.
    Cancel {
        id: u64,
    },
    /// This machine holds the letter the hub numbered `seq`, and all
    /// before it.
    Ack {
        seq: u64,
    },
    /// The record of a machine joining with an invitation, once it has the
    /// invitation (`Down::Endorsement`).
    Admit {
        record: Signed,
    },
    /// A question for the hub itself.
    Ask {
        id: u64,
        question: Question,
    },
    /// On a relay session: `name` connected below this relay, with its
    /// letters where they stand. `tag` is this relay's number for the
    /// attachment, carried by every frame of it, so a frame of an earlier
    /// attachment of the same machine is told apart.
    Attach {
        name: MachineName,
        tag: u64,
        letters: Letters,
    },
    /// On a relay session: the attachment `tag` of `name` ended.
    Detach {
        name: MachineName,
        tag: u64,
    },
    /// On a relay session: a frame from `name`, which the relay checked is
    /// the machine on that connection below it.
    For {
        name: MachineName,
        tag: u64,
        frame: Box<Up>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Intent {
    /// A member's control connection. `proof` is its key's signature of the
    /// TLS exporter, so the signature proves nothing on any other connection.
    Session { key: PublicKey, proof: Signature, letters: Letters },
    /// A machine joining: the founder with the hub's founding secret, or an
    /// invited machine with its admission inside the record.
    Join { record: Signed, founding: Option<Secret> },
    /// A machine holding an invitation's secret: `proof` is the invitation
    /// key's signature of the TLS exporter. The hub answers with the
    /// invitation, and the machine then sends its record (`Up::Admit`).
    Invited { invite: PublicKey, proof: Signature },
    /// One end of a data connection. A relay opening it upward for a
    /// machine below it names that machine in `on_behalf`; `key` and
    /// `proof` are then the relay's own.
    Data {
        key: PublicKey,
        proof: Signature,
        ticket: Signed,
        end: End,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_behalf: Option<MachineName>,
    },
    /// A relay's session with its upstream, carrying the sessions of the
    /// machines below it. `proof` is the relay's key's signature of the TLS
    /// exporter, as for a member's session.
    Relay { key: PublicKey, proof: Signature },
    /// Whether this upstream takes sessions now: answered `Down::Ready`,
    /// or refused, and closed. A machine on a fallback upstream asks its
    /// preferred one.
    Probe,
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

/// A transfer between two members, signed by its source: the hub pairs the
/// two ends that present the same ticket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub world: WorldId,
    /// Random, chosen by the source.
    pub stream: u64,
    pub source: MachineName,
    pub sink: MachineName,
}

/// What a transfer carries, said in a sealed call or answer before the
/// bytes flow, so the sink can check every byte it takes; and the key the
/// transfer is sealed with (`crate::stream_crypto`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carried {
    pub ticket: Signed,
    pub key: crate::stream_crypto::StreamKey,
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
        world: WorldId,
        name: MachineName,
        version: String,
        taken: u64,
        letters: u64,
    },
    /// The machine joined as `name`.
    Joined {
        world: WorldId,
        name: MachineName,
    },
    /// The invitation the joining machine holds the key of, as its inviter
    /// signed it.
    Endorsement {
        endorsement: Signed,
    },
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
        message: Signed,
    },
    /// A call for this machine, numbered `call` by the hub.
    Incoming {
        call: u64,
        message: Signed,
    },
    /// How the call `id` this machine made is going.
    Status {
        id: u64,
        status: CallStatus,
    },
    /// The answer to the call `id` this machine made.
    Answered {
        id: u64,
        message: Signed,
    },
    /// The answer to the question `id`.
    Told {
        id: u64,
        answer: Result<Told, String>,
    },
    /// The members, or which of them are revoked, changed.
    MembersChanged,
    /// The relay session is open.
    Relaying {
        world: WorldId,
        name: MachineName,
    },
    /// On a relay session: a frame for `name`, in its attachment `tag`.
    To {
        name: MachineName,
        tag: u64,
        frame: Box<Down>,
    },
    /// This upstream takes sessions now (`Intent::Probe`).
    Ready,
    /// A town this machine spans, or one an item was just sealed for it in,
    /// has new items in its ledger.
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
    #[error("this machine is not a member of the World")]
    NotMember,
    #[error("this machine's key was revoked by {by} at {}: {reason}", .at.strftime("%Y-%m-%d %H:%M:%S UTC"))]
    Revoked { by: MachineName, at: Timestamp, reason: String },
    #[error("the hub did not admit this machine: {reason}")]
    NotAdmitted { reason: String },
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
    /// That machine's connection ended before it answered; whether it did
    /// what was asked is not known.
    Lost,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Question {
    /// Every member's record, every revocation, and who is connected.
    Members,
    /// Hold this endorsement until a machine joins with it or it expires.
    Invite { endorsement: Signed },
    /// A member revokes a key.
    Revoke { revocation: Signed },
    /// A member's record again, with the same key: to add its sealing key.
    Republish { record: Signed },
    /// A member with management rights sets what a member may do.
    Grant { grant: Signed },
    /// A member with management rights sets where a member connects.
    Route { route: Signed },
    /// Keep this item, a sealed entry or a sealed copy of an entry's key, in
    /// its town's ledger (`crate::ledger`).
    LedgerWrite { item: Signed },
    /// The towns this machine spans, each with the number of its latest
    /// item.
    LedgerTowns,
    /// Every town of the World, each with the number of its latest item.
    /// This does not subscribe the asker to replication of those towns.
    LedgerCatalog,
    /// An explicitly queried town, including one this machine does not span.
    LedgerQuery { town: TownId, after: u64 },
    /// A town's items after the one numbered `after`, as many as fit in one
    /// frame.
    LedgerRead { town: TownId, after: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Told {
    Members {
        records: Vec<Signed>,
        revocations: Vec<Signed>,
        grants: Vec<Signed>,
        #[serde(default)]
        routes: Vec<Signed>,
        presence: Vec<Presence>,
        /// When the latest grant, route or revocation the hub holds was made:
        /// a new one is dated after it, so that every machine, taking them in the
        /// order they were made, takes them in the order the hub took them,
        /// whatever the members' clocks.
        #[serde(default)]
        latest: Option<Timestamp>,
    },
    Done,
    /// The item is the town's item `seq`.
    Written {
        seq: u64,
    },
    Towns {
        towns: Vec<(TownId, u64)>,
    },
    /// Items in order, each with its number; `more` when others follow.
    Entries {
        entries: Vec<(u64, Signed)>,
        more: bool,
    },
}

/// Whether a member is connected, as the hub sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub name: MachineName,
    pub online: bool,
    /// When it connected, if it is online; when it was last connected, if
    /// not and the hub knows.
    pub since: Option<Timestamp>,
}

/// Encodes a frame for the wire.
pub fn encode(frame: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(frame).expect("frames always serialize")
}
