//! Negotiated Yjs v1 document synchronization on a shared watch connection.
//! Input acknowledgements prove database commit, not merely frame reception.

use super::{QueuedCommand, Uploading};
use crate::{MachineName, NoticeId, access::Written, attachments::Attachment, ipc::Failure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

pub const VERSION: u32 = 1;

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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    State { version: u32, state: Box<State>, documents: Vec<Update> },
    Update { revision: u64, document: Update },
    Saved { number: u64, document: String, revision: u64, document_revision: u64 },
    Refused { number: u64, failure: Failure },
}
