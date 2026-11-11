//! Negotiated Yjs v1 document synchronization on a shared watch connection.
//! Input acknowledgements prove acceptance and database commit, not disk sync.
//! Clients retain their Yjs replica for state-vector repair after reconnecting.

use super::{QueuedCommand, Uploading};
use crate::{MachineName, NoticeId, access::Written, attachments::Attachment, ipc::Failure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

pub const VERSION: u32 = 1;
/// Bounds decoding before editor locks while admitting 30,000-scalar replacements
/// with fragmented UTF-16 deletion ranges as well as four-byte pasted text.
pub const MAX_UPDATE_BYTES: usize = 1024 * 1024;
/// A fresh frozen body of at most 30,000 scalars, with codec framing.
pub const MAX_SNAPSHOT_BYTES: usize = 128 * 1024;
/// At most 30,000 captured scalars, including fragmented UTF-16 identity ranges.
pub const MAX_TAKE_BYTES: usize = 1024 * 1024;

/// A Yjs v1 update or state vector, encoded as base64 in JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vector {
    pub document: String,
    pub vector: Bytes,
}

/// Absence requests the legacy composer protocol. Version is confirmed by State.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub version: u32,
    #[serde(default)]
    pub known: Vec<Vector>,
}

/// Attachments and legacy upload epochs are metadata, independent of text clocks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub revision: u64,
    pub epoch: u64,
    pub attachments: Vec<Attachment>,
    pub uploads: Vec<Uploading>,
    pub files_on: Option<Written>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queued {
    pub id: NoticeId,
    pub from: MachineName,
    pub document: Document,
    pub editing: bool,
    pub sending: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub revision: u64,
    pub draft: Document,
    pub queue: Vec<Queued>,
    pub commands: Vec<QueuedCommand>,
    pub unobserved: Vec<Queued>,
    pub held: Option<String>,
    pub awaiting_turn: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<Operation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    pub document: String,
    pub revision: u64,
    pub update: Bytes,
}

/// One serial input producer per subscription. Repeated text updates are
/// intrinsically idempotent; a lost acknowledgement never requires a new edit ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Input {
    Update { number: u64, document: String, update: Bytes },
    Mutate { number: u64, mutation: Box<super::Mutation> },
}
impl Input {
    pub fn number(&self) -> u64 {
        match self {
            Self::Update { number, .. } | Self::Mutate { number, .. } => *number,
        }
    }
}
/// A send is accepted after its capture, flight, and receipt commit together.
/// The same operation later carries dispatch's result on the state stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub operation: String,
    pub delivery: Option<NoticeId>,
    pub accepted: bool,
    pub landed: Option<crate::residents::Landed>,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    Result {
        number: u64,
        revision: u64,
        operation: Operation,
    },
    State {
        version: u32,
        state: Box<State>,
        documents: Vec<Update>,
        /// Authority vectors let a retained replica repair accepted, unsynced text.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        vectors: Vec<Vector>,
        /// Native mutation receipts and bounded send acceptance are supported.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        mutations: bool,
    },
    Update {
        revision: u64,
        document: Update,
    },
    /// The update is committed and visible; a text-only commit may still await disk sync.
    Saved {
        number: u64,
        document: String,
        revision: u64,
        document_revision: u64,
    },
    Refused {
        number: u64,
        failure: Failure,
    },
}
