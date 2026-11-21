//! The second version's asset sync on the wire (design §7): what a device
//! asks the hub, what the hub answers, and what it tells unasked. Carried
//! in `Question::Assets`, `Told::Assets` and `Down::Assets`, and sent only
//! when both sides declare `asset_sync`, its value the protocol version.
//!
//! Frames go as they are kept, never decoded and framed again; a frame whose
//! records are over 64 KiB, and every object's bytes, go over a data
//! connection instead (§7.1). The hub's words are its service key's,
//! bound to its chain head (`fastctx_net::sign::hub_word`).

use crate::access::TownId;
use crate::{AccountName, MachineName};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

/// The protocol version a side speaks. A hub declares as `asset_sync` the
/// newest it speaks; a device opens in the newest both speak, its own or the
/// hub's, none older than `OLDEST`, and the hub answers in it where it still
/// speaks it, refusing it where it does not. A later version is what any new
/// reader class, holder class, receipt or message comes with: a hub sends a
/// device nothing its version does not know (sync v2 freeze spec, 1). The
/// second brings proofs of holding by challenge (design §28.45); the third,
/// frames of a stream taken several at a time (`MANY`); the fourth, asset
/// lines (`LINES`); the fifth, a conversation's streams kept by the machine
/// that orders them and taken from it (`DIRECT`); the sixth, a
/// conversation's scope described for the account of each opener's name it
/// was begun for alone (`BORN`); the seventh, the objects a machine holds of
/// the conversation streams it orders vouched for to the hub (`VOUCH`).
pub const VERSION: u32 = 7;

/// The oldest version a side still speaks.
pub const OLDEST: u32 = 1;

/// The version from which a device proves it holds an object by answering a
/// challenge, not by sending every byte again (design §6.5, §28.45).
pub const CHALLENGES: u32 = 2;

/// The version from which a device sends a hub several frames of a stream
/// in one request (`Request::TakeMany`), which the hub writes with one disk
/// synchronization (§18 E2).
pub const MANY: u32 = 3;

/// The version from which a device keeps asset lines open to its hub
/// (`crate::asset_lines`): frames and objects go on them one after another,
/// with no ticket and no connection of their own.
pub const LINES: u32 = 4;

/// The version from which what a machine orders of a conversation stays on
/// that machine (`crate::asset_peers`): the machines that hold a copy take
/// its frames and objects from it, and the hub keeps who orders each stream
/// and who may read it, and none of what the streams hold.
pub const DIRECT: u32 = 5;

/// The version from which a device says, describing a conversation's scope,
/// which account of each opener's name it was begun for
/// (`Request::Describe`'s `born`), and the hub describes it for no other
/// (`BEGUN_FOR_ANOTHER`).
pub const BORN: u32 = 6;

/// What a hub says refusing to describe a scope begun for an earlier
/// account of an opener's name than the one there now; the opener's key
/// (`account:NAME`) follows.
pub const BEGUN_FOR_ANOTHER: &str = "begun for an earlier account of the name of ";

/// How far a machine's clock and its hub's may differ, in milliseconds, as
/// a scope is judged begun before an account was opened (`Request::Describe`'s
/// `begun`): an account opened later than that past this did not begin it.
pub const BEGUN_CLOCKS: i64 = 5 * 60 * 1000;

/// The version from which a device vouches to the hub for the objects it
/// holds of the conversation streams it orders (`Request::Vouch`): the hub
/// may let go of its own copy of an object every reference of which is so
/// vouched for, the machine the conversation lives on keeping it.
pub const VOUCH: u32 = 7;

/// The most objects one `Vouch` names.
pub const VOUCHED_A_TIME: usize = 512;

/// The most frames one `TakeMany` carries.
pub const FRAMES_A_TAKE: usize = 16;

/// The most bytes of frame bodies one `TakeMany` carries: each carried on
/// the control connection, none travelling apart.
pub const MANY_BYTES: usize = 1 << 20;

/// What a proof of holding begins with: the answer to a challenge is
/// `SHA256(PROOF_TAG ‖ nonce ‖ every byte of the object)` (§6.5).
pub const PROOF_TAG: &[u8] = b"fastctx-pop/1";

/// The most a frame's body carried on the control connection holds (§7.1);
/// a larger one travels apart, on a data connection.
pub const INLINE_FRAME_BYTES: usize = 64 * 1024;

/// The most a frame's body that travels apart holds: a frame's records are
/// at most 1 MiB before they are compressed.
pub const APART_FRAME_BYTES: u64 = 2 * 1024 * 1024;

/// The bytes of each chunk an object travels in but its last (§7.3).
pub const CHUNK_BYTES: u64 = 1024 * 1024;

/// How a call or a letter between machines is refused while the hub's
/// assets are sealed, the seal's reason after it (design §28.41).
pub const SEALED_RELAY: &str = "the hub's assets are sealed, so nothing goes between machines until they are opened: ";

/// Bytes, as base64 in JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bytes(pub Vec<u8>);

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        STANDARD.decode(value).map(Self).map_err(D::Error::custom)
    }
}

impl Bytes {
    /// The bytes as a digest or key, when there are 32 of them.
    pub fn array(&self) -> Option<[u8; 32]> {
        self.0.as_slice().try_into().ok()
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<[u8; 32]> for Bytes {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes.to_vec())
    }
}

/// A frame as kept: its header, its ordering device's signature and its
/// compressed records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub header: Bytes,
    pub signature: Bytes,
    pub body: Bytes,
    /// The length of a body that travels apart, on a data connection
    /// (§7.1): `body` is then empty.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub apart: u64,
}

impl Frame {
    /// A frame whose body is carried with it.
    pub fn whole(header: Vec<u8>, signature: Vec<u8>, body: Vec<u8>) -> Self {
        Self { header: header.into(), signature: signature.into(), body: body.into(), apart: 0 }
    }

    /// Whether its body is too large to be carried on the control
    /// connection.
    pub fn too_large(&self) -> bool {
        self.body.0.len() > INLINE_FRAME_BYTES
    }
}

/// An object a record refers to: its hash and length.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    pub hash: Bytes,
    pub size: u64,
}

/// A fact the hub signed, and its word on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub fact: Bytes,
    pub word: Bytes,
}

/// Who reads a scope (§2.1): a reference to facts of the directory, judged
/// against the directory as it stands at each request, never a list of
/// names copied into the scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "readers", rename_all = "snake_case")]
pub enum Readers {
    /// A hosted conversation: those who opened it and, unless it was born
    /// assisted, those who hold the machine it was born on (its owner and
    /// the accounts it is shared with whole).
    Conversation {
        openers: Vec<AccountName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        born_on: Option<MachineName>,
    },
    /// A project's readers: its owner, the accounts it is shared with, and
    /// those who hold the machine it was opened on.
    Project { town: TownId },
    /// An account alone: its settings, its usage.
    Account { account: AccountName },
    /// What several accounts own together (§6.2): the members of the body —
    /// a machine shared whole, its owner and those it is shared with — and
    /// the account that opened it.
    Body { body: String, opener: AccountName },
    /// The hub's own: its super administrators, as they are (§14).
    Hub,
    /// What an account does on a machine, read by the account for as long
    /// as it can work on that machine (§14: its MCP commands' logs).
    Working { account: AccountName, machine: MachineName },
}

/// Which devices may hold a class of a scope's streams (§2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Holders {
    /// Any device acting for a reader, an assisting host among them.
    Readers,
    /// A device a reader is logged in on: not a host assisting one.
    Logged,
}

/// A class of a scope's streams: those whose names start with `prefix`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamClass {
    pub prefix: String,
    pub holders: Holders,
}

/// A scope's descriptor (§2.1), as the hub signs it: its key, its owner
/// library, who reads it, its stream classes, its revision, and the hub's
/// chain number when it was written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    pub scope: String,
    /// The hub that described it: a scope of one hub is no other's.
    pub hub: String,
    pub owner: String,
    pub readers: Readers,
    /// Who each account, machine and project the readers name was when the
    /// scope was described (`account:NAME`, `machine:NAME`,
    /// `project:TOWN`): one deleted and made again under the same name reads
    /// nothing of it.
    pub born: std::collections::BTreeMap<String, String>,
    pub classes: Vec<StreamClass>,
    pub revision: u64,
    pub seq: u64,
}

impl Readers {
    /// Where the keys of scopes of these readers fall on the hub `hub`:
    /// whoever may begin such a scope names it under this, so knowing
    /// another's key takes nothing from it (§2.1). An account's names the
    /// hub too: two hubs' accounts of one name are two owners, whose scopes
    /// a machine on both holds apart (BB11).
    pub fn key_prefix(&self, hub: &str) -> String {
        match self {
            // Under the hub too: a device on two networks holds two machines
            // of one name apart (§29, 2.8).
            Self::Conversation { born_on: Some(machine), .. } => format!("c/{hub}/{machine}/"),
            Self::Conversation { openers, born_on: None } => {
                format!("c/{hub}/-/{}/", openers.first().map(|opener| opener.as_str()).unwrap_or_default())
            }
            Self::Project { town } => format!("p/{town}/"),
            Self::Account { account } => format!("a/{hub}/{account}/"),
            Self::Body { body, .. } => format!("b/{body}/"),
            Self::Hub => format!("h/{hub}/"),
            // Digested, so a long account and machine name never make a key
            // longer than a scope's (OO18).
            Self::Working { account, machine } => {
                use sha2::{Digest as _, Sha256};
                let digest = Sha256::digest(format!("{account}\0{machine}").as_bytes());
                let named: String = digest[..16].iter().map(|byte| format!("{byte:02x}")).collect();
                format!("w/{hub}/{named}/")
            }
        }
    }
}

/// The classes of a conversation's scope (§2.3): a host assisting one of
/// its readers holds what it works with, not the plan, notes, goals and
/// sentinels the hub keeps of it (L12).
pub fn conversation_classes() -> Vec<StreamClass> {
    let mut classes = vec![StreamClass { prefix: String::new(), holders: Holders::Readers }];
    classes.extend(
        ["plan", "history", "goals", "sentinels"]
            .into_iter()
            .map(|prefix| StreamClass { prefix: prefix.to_string(), holders: Holders::Logged }),
    );
    classes
}

/// The classes of an account's records' scope: held where the account is
/// logged in, but what a machine did, ran and spent for it, which a machine
/// working for it writes, held there too (L12, §28.58).
pub fn records_classes() -> Vec<StreamClass> {
    let mut classes = vec![StreamClass { prefix: String::new(), holders: Holders::Logged }];
    classes.extend(
        ["activity", "usage", "jobs", "output"]
            .into_iter()
            .map(|prefix| StreamClass { prefix: prefix.to_string(), holders: Holders::Readers }),
    );
    classes
}

/// The classes a scope of `readers` under `key` is described with: the
/// same wherever it is proposed, and the only ones the hub describes it
/// with, so the first device to describe one cannot widen who holds it
/// (AS09).
pub fn classes_for(readers: &Readers, key: &str) -> Vec<StreamClass> {
    let whole = |holders| vec![StreamClass { prefix: String::new(), holders }];
    match readers {
        Readers::Conversation { .. } => conversation_classes(),
        // A machine's log, read by its owner and those it is shared with
        // whole, held where they are logged in; a conversation opened on
        // it, as any conversation.
        Readers::Body { .. } if key.ends_with("/log") => whole(Holders::Logged),
        // What a machine shared whole did and spent, for any account: held
        // as an account's records are (§29, 2.5).
        Readers::Body { .. } if key.ends_with("/records") => records_classes(),
        Readers::Body { .. } => conversation_classes(),
        Readers::Account { .. } => records_classes(),
        Readers::Project { .. } => whole(Holders::Readers),
        Readers::Hub => whole(Holders::Logged),
        Readers::Working { .. } => whole(Holders::Readers),
    }
}

/// Whether the scope `key` is a conversation's: one opened on a machine, or
/// on a machine shared whole, that machine's log and records apart.
pub fn of_conversation(key: &str) -> bool {
    key.starts_with("c/") || (key.starts_with("b/") && !key.ends_with("/log") && !key.ends_with("/records"))
}

/// What the hub signs to say the device whose key is `device` holds each
/// scope of `granted` until `until`, the hub's milliseconds (`Reply::Permits`).
pub fn permit_statement(device: &[u8], until: i64, granted: &[String]) -> Vec<u8> {
    let mut statement = b"fastctx-asset-permit/1".to_vec();
    let mut field = |bytes: &[u8]| {
        statement.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        statement.extend_from_slice(bytes);
    };
    field(device);
    field(&until.to_le_bytes());
    field(&(granted.len() as u32).to_le_bytes());
    for scope in granted {
        field(scope.as_bytes());
    }
    statement
}

/// The key of the scope an account's records are kept in on `hub`: by when
/// it was opened too, so one made again under the same name has its own.
pub fn records_key(hub: &str, account: &AccountName, opened_ms: i64) -> String {
    format!("{}records-{opened_ms}", Readers::Account { account: account.clone() }.key_prefix(hub))
}

impl Descriptor {
    /// The class `stream` falls in: the longest prefix that names it.
    pub fn class(&self, stream: &str) -> Option<&StreamClass> {
        self.classes.iter().filter(|class| stream.starts_with(&class.prefix)).max_by_key(|class| class.prefix.len())
    }
}

/// The latest chain head a device saw, with the statement and the word
/// that carried it (§17, item 4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub seq: u64,
    pub head: Bytes,
    pub statement: Bytes,
    pub word: Bytes,
}

/// What a receipt says, as a device reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "said", rename_all = "snake_case")]
pub enum Said {
    Applied { change: u64, prefix: Bytes },
    Parked { sequence: u64, accumulated: Bytes },
}

/// A receipt not yet witnessed, as a device reports it when a session
/// opens (§17, item 4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reported {
    pub scope: String,
    pub stream: String,
    pub epoch: u64,
    pub first: u64,
    pub digest: Bytes,
    #[serde(flatten)]
    pub said: Said,
    pub word: Bytes,
}

/// Where a stream stands on a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub stream: String,
    pub sequence: u64,
    pub head: Bytes,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// What a device asks the hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum Request {
    /// Opens a session (§7.1): the latest chain head the device saw, and
    /// every receipt of its not yet witnessed (§17).
    Open {
        version: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seen: Option<Seen>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reported: Vec<Reported>,
        /// The service key this machine pinned for the hub: a hub whose
        /// keyring lacks it was put back to before a rotation (§17).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pinned: Option<Bytes>,
        /// The hub's signed rotations that lead to the pinned key, as this
        /// machine was given them: what shows a key the keyring lacks is
        /// the hub's own successor, not one a machine made.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        lineage: Vec<Rotation>,
    },
    /// The hub's word on a frame, from its signed header, if it has one
    /// (§6.5, step 1).
    Answer { scope: String, header: Bytes, signature: Bytes },
    /// A frame for the hub to take (§6.5, step 3), with the objects the
    /// device proved it holds. A body that travels apart was staged under
    /// the ticket `staged` (`Stage`).
    Take {
        scope: String,
        frame: Frame,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        proven: Vec<Bytes>,
        #[serde(default, skip_serializing_if = "is_zero")]
        staged: u64,
    },
    /// Frames of one stream of `scope` for the hub to take, in their order,
    /// each as `Take` takes one without objects proved or a body staged
    /// apart: answered with the hub's word on each, up to the first not
    /// applied or parked (`Reply::ReceivedMany`). From version `MANY` on.
    TakeMany { scope: String, frames: Vec<Frame> },
    /// Room for a frame body of `size` bytes of `scope`, too large for the
    /// control connection: a data connection under the ticket the hub
    /// answers sends it, in chunks, before the frame is taken (§7.1).
    Stage { scope: String, size: u64 },
    /// The scopes this machine may hold, a page at a time in the order they
    /// were described, from after `after` (§6.6).
    Holdings {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<HoldingsAt>,
    },
    /// Where the device's streams of a scope stand in its holding period
    /// `holding` (§6.6, §7.4).
    Cursors { scope: String, holding: i64, streams: Vec<Cursor> },
    /// A frame of a stream the hub orders, as the hub signed it, handed back
    /// while the hub is sealed for a recovery, with the bytes of the objects
    /// its records refer to (hash, bytes): what the hub put back lost past
    /// where it was put back (§28.38).
    HandBack {
        scope: String,
        stream: String,
        frame: Frame,
        #[serde(default)]
        objects: Vec<(Bytes, Bytes)>,
    },
    /// Up to `limit` frames of a stream, from the one holding `from` (§6.6).
    Frames { scope: String, stream: String, from: u64, limit: u32 },
    /// A stream's signed epochs and gap facts (§8).
    Succession { scope: String, stream: String },
    /// A stream's newest items as they stand at its head, with the hub's
    /// word on them (§9, §28.2).
    Window { scope: String, stream: String },
    /// Up to `limit` items of a stream below `before` (order key and
    /// identity; from the top when none), newest first, as they stand now
    /// (§9).
    Older {
        scope: String,
        stream: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<(Bytes, Bytes)>,
        limit: u32,
    },
    /// Describes a scope this device begins (§2.1): the hub signs its
    /// descriptor, or answers the one it has. From version `BORN` on, `born`
    /// says who each opener of a conversation's scope was as it was begun —
    /// `account:NAME` to when that account was opened, in milliseconds — and
    /// a hub describing it anew describes it for no later account of a name.
    Describe {
        scope: String,
        readers: Readers,
        classes: Vec<StreamClass>,
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        born: std::collections::BTreeMap<String, String>,
        /// When the device began the scope, in its milliseconds: an opener
        /// `born` says nothing of is judged by it (`BEGUN_CLOCKS`). From
        /// version `BORN` on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        begun: Option<i64>,
    },
    /// The streams of a scope this device holds, each with the sequence the
    /// hub's applied prefix reaches next (§6.6).
    Streams { scope: String },
    /// What changed in `owner`'s library after the change number `after`
    /// and below `before` (the latest, when none), of `class` alone when
    /// given, newest first, at most `limit` streams (§28.1).
    Changes {
        owner: String,
        after: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<u8>,
        limit: u32,
    },
    /// A new epoch of a stream this device orders, after the hub refused
    /// one of its frames: what remains is committed again from `next`,
    /// chained from `head` (§6.5).
    Epoch { scope: String, stream: String, next: u64, head: Bytes },
    /// This device takes over ordering `stream`, which another ordered: a
    /// conversation came to live here (design §8, §28.10). Under the
    /// predecessor's `release` naming this machine, or, with `adopt`, as the
    /// account that opened the scope, logged in here, gives it to this
    /// machine, the device it lived on lost: the hub hands it over where the
    /// predecessor's frames it holds reach, and answers the new epoch's
    /// descriptor, or says it was handed on since.
    Takeover {
        scope: String,
        stream: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release: Option<Release>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        adopt: bool,
    },
    /// Puts an object of `scope` to the hub (§7.3), its chunks' digests
    /// given (none for one of a single chunk): the hub answers which chunks
    /// it lacks and the ticket a data connection sends them under. With
    /// `prove`, every byte is sent though the hub holds it: the device
    /// proves it holds the object (§6.5).
    Put {
        scope: String,
        object: ObjectRef,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        manifest: Vec<Bytes>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        prove: bool,
    },
    /// Takes an object of `scope` from the hub, from the chunk `from`: the
    /// hub answers its chunks' digests and the ticket a data connection
    /// brings them under.
    Get { scope: String, object: ObjectRef, from: u64 },
    /// This machine cleared what the accounts its shares wait for may not
    /// see, at its holding revision `revision` (§6.3), under its key
    /// (`cleared_statement`).
    Cleared { revision: u64, proof: Bytes },
    /// Where this device stands (design §28.9): the hub keeps the latest
    /// report of each machine.
    Report { report: crate::sync_status::SyncReport },
    /// How `account`'s machines stand, over the last `hours` (design §28.9):
    /// asked by a machine the account is logged in on.
    ///
    /// With `machine`, that machine's line alone, at its full step; without,
    /// every machine's, coarser where the account has many machines, so a
    /// page of a hundred is not a hundred full lines a minute. A hub that
    /// knows no `machine` gives every line as it always did.
    SyncStatus {
        account: crate::AccountName,
        hours: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<crate::MachineName>,
    },
    /// Asks for a challenge, to prove it holds `object`, which the hub asked
    /// it lately to prove for a frame it sent (design §28.45). From version
    /// `CHALLENGES` on.
    Challenge { object: ObjectRef },
    /// Answers a challenge with `SHA256(PROOF_TAG ‖ nonce ‖ every byte)`;
    /// asked again with the same nonce and answer, how the proof stands
    /// (design §28.45). From version `CHALLENGES` on.
    Proof { nonce: Bytes, answer: Bytes },
    /// Opens the first epoch of `stream`, which this device orders and
    /// sends the hub no frame of: answered with the epoch's descriptor, the
    /// one the hub signed already where it did. From version `DIRECT` on.
    Begin { scope: String, stream: String },
    /// `stream`, which this device orders and keeps, reaches `next`: the
    /// machines that hold the scope are told it moved. From version `DIRECT`
    /// on. `epoch`, the epoch it is written in here, where it begins and the
    /// chain value it follows: the hub takes it only where its own last
    /// epoch of the stream is that one (none from a device before 1.0.1-beta.23).
    Stirred {
        scope: String,
        stream: String,
        next: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        epoch: Option<StirredIn>,
    },
    /// The hub's word on which of `scopes` this device holds, for the
    /// machines that keep their streams to give them by. From version
    /// `DIRECT` on.
    Permits { scopes: Vec<String> },
    /// This device, ordering `stream` of the conversation scope `scope` in
    /// `epoch`, holds whole each object of `hashes` (at most
    /// `VOUCHED_A_TIME`) its frames refer to, the oldest of them read back
    /// whole `read_ago` milliseconds ago. `anew`, what it vouched for of the
    /// stream before is set aside first: it tells the stream from its start.
    /// From version `VOUCH` on.
    Vouch {
        scope: String,
        stream: String,
        epoch: u64,
        hashes: Vec<Bytes>,
        #[serde(default)]
        read_ago: u64,
        #[serde(default)]
        anew: bool,
    },
}

/// The epoch a stream is written in on the device that orders it, as
/// `Request::Stirred` says it: its number, where it begins, and the chain
/// value its first frame follows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StirredIn {
    pub epoch: u64,
    pub start: u64,
    pub follows: Bytes,
}

/// How a proof of holding by challenge stands (design §28.45).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofState {
    /// Queued or being worked out: ask again.
    Working,
    /// Proved: the hub takes the frame as from a device that sent every
    /// byte of the object lately.
    Proven,
    /// The answer is not the hub's: proved by sending every byte, if at all.
    Wrong,
    /// The hub's own bytes are not the object's: sent every byte again.
    Damaged,
}

/// A device's release of a stream it orders to the machine `to`, whose
/// device key is `to_key` (design §28.10): from its epoch `epoch`, having
/// written as far as `next`, chained to `head`, on the hub that hears it;
/// under its key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub epoch: u64,
    pub to: String,
    pub to_key: Bytes,
    pub next: u64,
    pub head: Bytes,
    pub signature: Bytes,
}

/// Where a listing of what a machine may hold goes on from: the place, in
/// the order scopes were described, of the last one looked at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldingsAt {
    pub seq: u64,
    pub scope: String,
}

/// A scope a machine may hold, with the hub's word on its descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub scope: String,
    pub descriptor: Fact,
    /// The prefixes of its stream classes this machine may hold now: what
    /// it may hold of a scope can change with no new descriptor (HH04).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<String>,
    /// Those logged in on this machine who read the scope, each as
    /// `account@opened` (milliseconds): a copy is read here only by one of
    /// them while still logged in, unlocked and the same account (AV01,
    /// AV02, AV09). Listed for a scope with a class only those logged in hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub readers: Vec<String>,
    /// Whether `readers` were listed while the body the scope is in had its
    /// machine gone from the network, its members frozen: only such a
    /// listing stands for a body whose machine is gone (AW01).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub frozen: bool,
}

/// The old service key's word that the new one follows it (§6.1),
/// over `fastctx-hub-key-rotation/1 ‖ old ‖ new`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rotation {
    pub old: Bytes,
    pub new: Bytes,
    pub word: Bytes,
}

/// What a machine signs saying it cleared at a holding revision (§6.3).
pub fn cleared_statement(machine: &str, revision: u64) -> Vec<u8> {
    let mut statement = b"fastctx-share-cleared/1".to_vec();
    statement.extend_from_slice(&(machine.len() as u32).to_le_bytes());
    statement.extend_from_slice(machine.as_bytes());
    statement.extend_from_slice(&revision.to_le_bytes());
    statement
}

/// What precedes a large object's chunk digests on its data connection: as
/// many as follow, 32 bytes each (§7.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestHead {
    pub count: u64,
}

/// The chunks the hub wants of an object put, as ranges [start, end).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wanted {
    pub ranges: Vec<(u64, u64)>,
}

/// What precedes each chunk's bytes on an object's data connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkHead {
    pub index: u64,
    pub size: u64,
}

/// One stream's latest change in an owner library: where it stands on the
/// hub, the bytes of its frames through its head, and when it changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedStream {
    pub scope: String,
    pub stream: String,
    pub next: u64,
    pub head: Bytes,
    pub bytes: u64,
    /// The change at which the hub last indexed the stream's records again
    /// (design §28.1): later frames leave it as it is.
    #[serde(default)]
    pub remade: u64,
    pub at: i64,
    /// The stream's last epoch the hub signed (design §28.10, I09).
    #[serde(default)]
    pub epoch: u64,
}

/// An item as a hub's index has it: identity, order key, the revision that
/// made it so, its kind and schema, its body and the objects it refers to
/// (hash and size).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub item: Bytes,
    pub order: Bytes,
    pub revision: u64,
    pub kind: u16,
    pub schema: u16,
    pub body: Bytes,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<(Bytes, u64)>,
}

/// Where a stream stands on the hub: the sequence its applied prefix
/// reaches next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamAt {
    pub stream: String,
    pub next: u64,
    /// The stream's last epoch on the hub: a device that knows an earlier
    /// one takes its descriptors, its frames' heads moved or not (I09, LL05).
    #[serde(default)]
    pub epoch: u64,
    /// How many times the hub made the stream again (§28.2): a light
    /// device's window or pages of an earlier making are taken anew though
    /// the head did not move (codex-058517ed #10, #11).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub remade: u64,
}

impl Request {
    /// The request's name as it goes on the wire, for what is told of it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::Answer { .. } => "answer",
            Self::Take { .. } => "take",
            Self::TakeMany { .. } => "take_many",
            Self::Stage { .. } => "stage",
            Self::Holdings { .. } => "holdings",
            Self::Cursors { .. } => "cursors",
            Self::HandBack { .. } => "hand_back",
            Self::Frames { .. } => "frames",
            Self::Succession { .. } => "succession",
            Self::Window { .. } => "window",
            Self::Older { .. } => "older",
            Self::Describe { .. } => "describe",
            Self::Streams { .. } => "streams",
            Self::Changes { .. } => "changes",
            Self::Epoch { .. } => "epoch",
            Self::Takeover { .. } => "takeover",
            Self::Put { .. } => "put",
            Self::Get { .. } => "get",
            Self::Cleared { .. } => "cleared",
            Self::Report { .. } => "report",
            Self::SyncStatus { .. } => "sync_status",
            Self::Challenge { .. } => "challenge",
            Self::Proof { .. } => "proof",
            Self::Begin { .. } => "begin",
            Self::Stirred { .. } => "stirred",
            Self::Permits { .. } => "permits",
            Self::Vouch { .. } => "vouch",
        }
    }

    /// What every asset request is for on the wire.
    pub fn purpose(&self) -> crate::sync::transfer::Purpose {
        use crate::sync::transfer::Purpose;
        match self {
            Self::Open { .. }
            | Self::Answer { .. }
            | Self::Take { .. }
            | Self::TakeMany { .. }
            | Self::Stage { .. }
            | Self::Holdings { .. }
            | Self::Cursors { .. }
            | Self::HandBack { .. }
            | Self::Frames { .. }
            | Self::Succession { .. }
            | Self::Window { .. }
            | Self::Older { .. }
            | Self::Describe { .. }
            | Self::Streams { .. }
            | Self::Changes { .. }
            | Self::Epoch { .. }
            | Self::Takeover { .. }
            | Self::Put { .. }
            | Self::Get { .. }
            | Self::Cleared { .. }
            | Self::Report { .. }
            | Self::SyncStatus { .. }
            | Self::Challenge { .. }
            | Self::Proof { .. }
            | Self::Begin { .. }
            | Self::Stirred { .. }
            | Self::Permits { .. }
            | Self::Vouch { .. } => Purpose::AssetSync,
        }
    }
}

/// A parked frame refused when its turn came.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    pub header: Bytes,
    pub why: String,
    pub word: Bytes,
}

/// The hub's word on a frame (§6.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "received", rename_all = "snake_case")]
pub enum Received {
    /// Applied with its change number and the scope's prefix there; and
    /// how many frames parked behind it were applied after it, up to the
    /// sequence `through`, and the one of them refused, if any.
    Applied {
        change: u64,
        prefix: Bytes,
        word: Bytes,
        #[serde(default, skip_serializing_if = "is_zero")]
        applied: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        through: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refused: Option<Refused>,
    },
    Parked { sequence: u64, accumulated: Bytes, word: Bytes },
    Rejected { why: String, from: u64, word: Bytes },
    /// Not held yet: send `from` to `to` first.
    Gap { from: u64, to: u64 },
    /// Not here yet: put these objects first.
    Missing { objects: Vec<ObjectRef> },
    /// Prove holding these by their whole bytes first.
    Prove { objects: Vec<ObjectRef> },
}

/// What the hub makes of one of a device's cursors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    pub stream: String,
    pub agrees: bool,
    /// When it does not: the ends of the hub's frames before it, latest
    /// first, each a sequence and the chain value there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ends: Vec<(u64, Bytes)>,
}

/// A checkpoint written outside the hub's directory covering a scope's
/// changes up to `change` and its parked frames up to `parked` (§17).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub scope: String,
    pub change: u64,
    pub parked: u64,
    pub seq: u64,
    pub head: Bytes,
    pub word: Bytes,
}

/// What a checkpoint's word says, as the hub signs it (§17): that a
/// checkpoint at chain number `seq` with head `head`, written outside the
/// hub's directory, covers `scope`'s changes up to `change` and its parked
/// frames up to `parked`.
pub fn checkpoint_statement(scope: &str, change: u64, parked: u64, seq: u64, head: &[u8; 32]) -> Vec<u8> {
    let mut bytes = b"fastctx-checkpointed/1".to_vec();
    bytes.extend_from_slice(&(scope.len() as u32).to_le_bytes());
    bytes.extend_from_slice(scope.as_bytes());
    bytes.extend_from_slice(&change.to_le_bytes());
    bytes.extend_from_slice(&parked.to_le_bytes());
    bytes.extend_from_slice(&seq.to_le_bytes());
    bytes.extend_from_slice(head);
    bytes
}

/// What the hub answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    /// The session is open: the hub's service key, its clock in
    /// milliseconds and its chain head now; and the checkpoints that cover
    /// receipts the device reported.
    Ready {
        key: Bytes,
        clock: i64,
        seq: u64,
        head: Bytes,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        checkpointed: Vec<Checkpoint>,
        /// Each rotation of the hub's service key, the old key's word on the
        /// new (§6.1): a machine follows them from the key it pinned.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rotations: Vec<Rotation>,
        /// What the machine saw or was given is of a history the hub put
        /// aside when it recovered (§17): it forgets that history's head and
        /// receipts, and gives back what the hub lacks.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        closed: bool,
    },
    /// The hub was put back and keeps every asset sealed until recovery
    /// ends (§17). Sealed for a recovery, the recovery generation it will
    /// reopen as: a device hands back what the hub signed past where it was
    /// put back (`HandBack`, §28.38).
    Sealed {
        why: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovery: Option<u64>,
    },
    /// What the hub made of a frame handed back to it while sealed for a
    /// recovery: kept for it to take back as it reopens, or not, and why.
    HandedBack {
        kept: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    /// This machine reads the scope no more, by the directory as it stands
    /// (§4.5, §6.3): it stops using what it holds of it.
    Revoked { scope: String },
    /// How many shares waiting at the revision a machine cleared at are in
    /// force now.
    Cleared { activated: u32 },
    Refused { why: String },
    Received { received: Received },
    /// The hub's word on each frame of a `TakeMany`, in order, up to the
    /// first not applied or parked: those after it it did not take.
    ReceivedMany { received: Vec<Received> },
    Word {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        received: Option<Received>,
    },
    Standing { streams: Vec<Standing> },
    /// Frames, those whose bodies travel apart brought in order, in chunks,
    /// by a data connection under `ticket`.
    Frames {
        frames: Vec<Frame>,
        #[serde(default, skip_serializing_if = "is_zero")]
        ticket: u64,
    },
    /// The ticket a staged frame body is sent under.
    Staged { ticket: u64 },
    /// Scopes this machine may hold; `next` where the listing goes on from,
    /// none at its end.
    Holdings {
        scopes: Vec<Held>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<HoldingsAt>,
        /// The last scope this answer looked at, given or not: where the
        /// next listing goes on from (AD10).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        through: Option<HoldingsAt>,
        /// What decides what any machine may hold, as a digest: when it is
        /// not what the machine last heard, a scope described long ago may be
        /// its to hold now, and it lists them all again.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        basis: String,
    },
    Succession { epochs: Vec<Fact>, gaps: Vec<Fact> },
    /// A scope's descriptor, as JSON, and the hub's word on it.
    Described { descriptor: Fact },
    Streams {
        streams: Vec<StreamAt>,
        /// Streams of the scope the hub has that this machine may hold no
        /// more (§2.3): it drops what it holds of them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        withheld: Vec<String>,
    },
    /// A page of what changed in an owner library (§28.1): the library's
    /// generation; the streams this device may hold among them, newest
    /// first; the latest change and when it was; where the next page goes on
    /// below, stepping past what this device may not hold as well.
    Changes { generation: String, streams: Vec<ChangedStream>, top: u64, top_at: i64, through: u64, more: bool },
    /// A stream's window (§28.2): at its head, the sequence its next frame
    /// takes, its chain value, the bytes of its frames and the change it was
    /// last indexed again at, in the owner library's generation; its newest
    /// items from `low` up (none: every item); and the hub's word on all of
    /// it.
    Window {
        generation: String,
        next: u64,
        head: Bytes,
        bytes: u64,
        remade: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        low: Option<(Bytes, Bytes)>,
        entries: Vec<Entry>,
        word: Bytes,
    },
    /// Items below a key as they stood when asked, with the sequence the
    /// stream's next frame took then, and whether more are below.
    Older {
        through: u64,
        entries: Vec<Entry>,
        more: bool,
        /// How many times the stream was indexed again as the hub answered
        /// (§29 3.3): a page of another making than the device's window is
        /// asked for again.
        #[serde(default, skip_serializing_if = "is_zero")]
        remade: u64,
    },
    /// The report was kept.
    Reported,
    /// How the account's machines stand.
    SyncStatus { status: Box<crate::sync_status::SyncStatus> },
    /// The new epoch's descriptor and the hub's word on it.
    Epoch { descriptor: Fact },
    /// A move asked again that was handed on since: the epochs after the one
    /// the machine took over in, which it takes as history alone.
    HandedOn { epochs: Vec<Fact> },
    /// The chunks the hub lacks of an object put, sent under `ticket`; none
    /// when it holds the object whole and nothing is to be proven.
    Put {
        ticket: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        missing: Vec<u64>,
    },
    /// An object's chunks' digests, brought under `ticket` from the chunk
    /// asked for.
    Get {
        ticket: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        manifest: Vec<Bytes>,
    },
    /// A challenge: the nonce to answer, and until when it stands, in
    /// milliseconds since the epoch (design §28.45).
    Challenge { nonce: Bytes, until: i64 },
    /// How a proof by challenge stands.
    Proof { proof: ProofState },
    /// What was said of a stream's moving was told on.
    Noted,
    /// The scopes, of those asked after, this device holds until `until`, the
    /// hub's milliseconds, and the hub's word on it (`permit_statement`).
    Permits { until: i64, granted: Vec<String>, word: Bytes },
    /// How many of the objects vouched for the stream's frames here refer
    /// to, and are kept as vouched for.
    Vouched { kept: u32 },
}

/// What the hub tells a device unasked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "push", rename_all = "snake_case")]
pub enum Push {
    /// A stream has frames to `next`: the device asks for those it lacks.
    Moved { scope: String, stream: String, next: u64 },
    /// A checkpoint covers receipts the device was given (§17).
    Checkpointed { checkpoint: Checkpoint },
    /// A control fact of a scope: an epoch, a gap, a descriptor.
    Fact { scope: String, fact: Fact },
    /// The device reads the scope no more (§4.5, §6.3).
    Revoked { scope: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_keep_their_shape() {
        let request = Request::Take {
            scope: "s".into(),
            frame: Frame::whole(vec![1], vec![2], vec![3]),
            proven: Vec::new(),
            staged: 0,
        };
        let text = serde_json::to_string(&request).unwrap();
        assert_eq!(text, r#"{"ask":"take","scope":"s","frame":{"header":"AQ==","signature":"Ag==","body":"Aw=="}}"#);
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), request);
        let reported = Reported {
            scope: "s".into(),
            stream: "t".into(),
            epoch: 0,
            first: 3,
            digest: vec![9].into(),
            said: Said::Applied { change: 7, prefix: vec![8].into() },
            word: vec![1].into(),
        };
        let text = serde_json::to_string(&reported).unwrap();
        assert_eq!(serde_json::from_str::<Reported>(&text).unwrap(), reported);
    }
}
