//! The resident machine's shared input and deferred messages.

pub mod sync;

use serde::{Deserialize, Serialize};

use crate::attachments::Attachment;
use crate::residents::Landed;
use crate::{MachineName, NoticeId};

/// A Unicode scalar's stable identity. Indices count scalars, not UTF-16 units.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CharacterId {
    pub operation: String,
    pub index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Character {
    pub id: CharacterId,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    /// Changes when the draft is sent. Edits to an earlier epoch fail explicitly.
    pub epoch: u64,
    pub text: String,
    pub characters: Vec<Character>,
    pub attachments: Vec<Attachment>,
    pub uploads: Vec<Uploading>,
    /// Legacy destination hint; new uploads keep their original locations.
    #[serde(default)]
    pub files_on: Option<crate::access::Written>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uploading {
    pub id: NoticeId,
    pub file: crate::attachments::UploadFile,
    pub error: Option<String>,
    /// The upload operation it arrives under: the device sending it knows
    /// its own. Older nodes do not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
}

/// Binds an attachment's entire upload lifetime to one shared draft.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadScope {
    pub operation: String,
    pub target: Target,
    pub epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadResult {
    pub attachment: Attachment,
    pub updated: Result<Updated, crate::ipc::Failure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queued {
    pub id: NoticeId,
    pub from: MachineName,
    pub draft: Draft,
    pub editing: bool,
    pub sending: bool,
    pub error: Option<String>,
}

/// A command the node took to do once a turn ends, where it stands among
/// the words that wait: after `after`, or before them all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedCommand {
    pub id: NoticeId,
    pub after: Option<NoticeId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Composer {
    pub revision: u64,
    pub draft: Draft,
    pub queue: Vec<Queued>,
    /// Moved like the words, by `Change::Move` with their ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<QueuedCommand>,
    /// Submitted messages with no observable outcome; retained separately and never resubmitted.
    pub unobserved: Vec<Queued>,
    /// The reason automatic release has stopped; it is never hidden by reconnecting.
    pub held: Option<String>,
    pub awaiting_turn: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Draft,
    Queued { id: NoticeId },
}

/// An operation ID is unique per client operation and reused only for an exact retry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mutation {
    pub operation: String,
    #[serde(flatten)]
    pub change: Change,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Change {
    /// Deletes only named characters. Concurrent inserts are never overwritten.
    Edit {
        target: Target,
        epoch: u64,
        after: Option<CharacterId>,
        delete: Vec<CharacterId>,
        insert: String,
    },
    Attach {
        target: Target,
        epoch: u64,
        attachment: Attachment,
    },
    Detach {
        target: Target,
        epoch: u64,
        attachment: NoticeId,
    },
    /// Retains an older client's destination hint without moving originals.
    FilesOn {
        target: Target,
        epoch: u64,
        files_on: Option<crate::access::Written>,
    },
    /// Sends the current shared draft, or appends it when the resident is busy.
    Send {
        epoch: u64,
    },
    /// Sends exactly the frozen replica's visible text. Its deletion leaves
    /// concurrent text on the authoritative document, whose identity stays stable.
    SendDocument {
        document: String,
        snapshot: sync::Bytes,
        attachments: Vec<NoticeId>,
    },
    /// Pauses a queued item while this watch is editing it. Closing the watch ends it.
    Editing {
        id: NoticeId,
        watch: String,
        editing: bool,
    },
    Delete {
        id: NoticeId,
    },
    /// Moves one item before another, or to the end. Sending items cannot move.
    Move {
        id: NoticeId,
        before: Option<NoticeId>,
    },
    /// Immediately delivers precisely this item, without advancing other queued items.
    SendQueued {
        id: NoticeId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Request {
    Read,
    Sync { input: Box<sync::Input> },
    Mutate { mutation: Box<Mutation> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Updated {
    pub composer: Composer,
    pub landed: Option<Landed>,
    /// The id the words this change sent are kept under on their way in,
    /// as `Watched::Now` names them. Older nodes do not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<NoticeId>,
}
