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

/// The protocol version a side declares as `asset_sync`.
pub const VERSION: u32 = 1;

/// The most a frame carried on the control connection holds (§7.1).
pub const INLINE_FRAME_BYTES: usize = 64 * 1024;

/// The bytes of each chunk an object travels in but its last (§7.3).
pub const CHUNK_BYTES: u64 = 1024 * 1024;

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
    /// Where the keys of scopes of these readers fall: whoever may begin
    /// such a scope names it under this, so knowing another's key takes
    /// nothing from it (§2.1).
    pub fn key_prefix(&self) -> String {
        match self {
            Self::Conversation { born_on: Some(machine), .. } => format!("c/{machine}/"),
            Self::Conversation { openers, born_on: None } => {
                format!("c/-/{}/", openers.first().map(|opener| opener.as_str()).unwrap_or_default())
            }
            Self::Project { town } => format!("p/{town}/"),
            Self::Account { account } => format!("a/{account}/"),
        }
    }
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
    },
    /// The hub's word on a frame, from its signed header, if it has one
    /// (§6.5, step 1).
    Answer { scope: String, header: Bytes, signature: Bytes },
    /// A frame for the hub to take (§6.5, step 3), with the objects the
    /// device proved it holds.
    Take {
        scope: String,
        frame: Frame,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        proven: Vec<Bytes>,
    },
    /// Where the device's streams of a scope stand in its holding period
    /// `holding` (§6.6, §7.4).
    Cursors { scope: String, holding: i64, streams: Vec<Cursor> },
    /// Up to `limit` frames of a stream, from the one holding `from` (§6.6).
    Frames { scope: String, stream: String, from: u64, limit: u32 },
    /// A stream's signed epochs and gap facts (§8).
    Succession { scope: String, stream: String },
    /// Describes a scope this device begins (§2.1): the hub signs its
    /// descriptor, or answers the one it has.
    Describe { scope: String, readers: Readers, classes: Vec<StreamClass> },
    /// The streams of a scope this device holds, each with the sequence the
    /// hub's applied prefix reaches next (§6.6).
    Streams { scope: String },
    /// A new epoch of a stream this device orders, after the hub refused
    /// one of its frames: what remains is committed again from `next`,
    /// chained from `head` (§6.5).
    Epoch { scope: String, stream: String, next: u64, head: Bytes },
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

/// Where a stream stands on the hub: the sequence its applied prefix
/// reaches next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamAt {
    pub stream: String,
    pub next: u64,
}

impl Request {
    /// What every asset request is for on the wire.
    pub fn purpose(&self) -> crate::sync::transfer::Purpose {
        use crate::sync::transfer::Purpose;
        match self {
            Self::Open { .. }
            | Self::Answer { .. }
            | Self::Take { .. }
            | Self::Cursors { .. }
            | Self::Frames { .. }
            | Self::Succession { .. }
            | Self::Describe { .. }
            | Self::Streams { .. }
            | Self::Epoch { .. }
            | Self::Put { .. }
            | Self::Get { .. }
            | Self::Cleared { .. } => Purpose::AssetSync,
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
    /// ends (§17).
    Sealed { why: String },
    /// This machine reads the scope no more, by the directory as it stands
    /// (§4.5, §6.3): it stops using what it holds of it.
    Revoked { scope: String },
    /// How many shares waiting at the revision a machine cleared at are in
    /// force now.
    Cleared { activated: u32 },
    Refused { why: String },
    Received { received: Received },
    Word {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        received: Option<Received>,
    },
    Standing { streams: Vec<Standing> },
    Frames { frames: Vec<Frame> },
    Succession { epochs: Vec<Fact>, gaps: Vec<Fact> },
    /// A scope's descriptor, as JSON, and the hub's word on it.
    Described { descriptor: Fact },
    Streams { streams: Vec<StreamAt> },
    /// The new epoch's descriptor and the hub's word on it.
    Epoch { descriptor: Fact },
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
            frame: Frame { header: vec![1].into(), signature: vec![2].into(), body: vec![3].into() },
            proven: Vec::new(),
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
