//! User attachments retain their original location on the sending computer.

use serde::{Deserialize, Serialize};

use crate::{Address, AgentName, NoticeId, ResidentId, TownName};

/// Where an upload belongs on the receiving machine. A new conversation
/// reserves its resident before any host starts; subsequent files use Draft.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UploadTarget {
    /// Keep bytes on the sending computer, or on `machine` for a node-less device.
    /// The operation is stable across retries; the inner target names the conversation.
    Sent {
        machine: crate::MachineName,
        target: Box<UploadTarget>,
        operation: String,
    },
    Resident {
        resident: ResidentId,
    },
    New {
        agent: AgentName,
        town: Option<TownName>,
    },
    Draft {
        resident: ResidentId,
        town: Option<TownName>,
    },
}

/// Description supplied by the uploader. Size and SHA-256 describe the
/// original bytes; media_type is the uploader's MIME type, not conversion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadFile {
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
}

/// A verified upload receipt. Pass it unchanged to Talk or Open. The node
/// resolves its own record by owner, town and id; no client path is trusted.
/// Town is where the upload happened, even after the resident moves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: NoticeId,
    pub owner: Address,
    /// The original conversation's home. Absent in historical receipts, where
    /// the holding machine was also the conversation's home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<Box<Address>>,
    pub town: Option<TownName>,
    pub path: String,
    #[serde(flatten)]
    pub file: UploadFile,
}

/// The existing byte upload may also complete a shared composer attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uploaded {
    pub attachment: Attachment,
    pub composer: Option<Result<crate::composer::Updated, crate::ipc::Failure>>,
}

impl UploadTarget {
    /// The conversation target without the transport's storage policy.
    pub fn conversation_target(&self) -> &Self {
        match self {
            Self::Sent { target, .. } => target,
            target => target,
        }
    }
}

impl Attachment {
    pub fn conversation(&self) -> &Address {
        self.conversation.as_deref().unwrap_or(&self.owner)
    }

    pub fn is_image(&self) -> bool {
        self.file.media_type.starts_with("image/")
    }
}

/// An upload receipt, or one user image in the resident's native history.
/// Image counts only image/localImage blocks, starting at zero. Codex
/// message counts distinct native user submissions in its turn, before
/// World envelopes are restored into separate messages. Host paths are
/// never supplied; Claude uses its durable user-message UUID. Image order
/// follows reloaded host history: original order for canonical Codex
/// messages, inline images then local images for legacy Codex history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttachmentSource {
    Upload(Attachment),
    HostImage { owner: Address, host_image: HostImage },
    ToolImage { owner: Address, tool_image: ToolImage },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "host", rename_all = "snake_case")]
pub enum HostImage {
    Codex { turn: String, message: u32, image: u32 },
    Claude { message: String, image: u32 },
}

/// An image a tool gave back, as its host's record keeps it: the tool's
/// item and the image's place among the images of its output, counting
/// from zero. Codex finds an item within its turn; Claude Code's tool ids
/// are unique in a session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolImage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    pub item: String,
    pub image: u32,
}

impl AttachmentSource {
    /// Remote originals have one small history image at their conversation's home.
    pub fn thumbnail_owner(&self) -> &Address {
        match self {
            Self::Upload(attachment) => attachment.conversation(),
            _ => self.owner(),
        }
    }

    pub fn owner(&self) -> &Address {
        match self {
            Self::Upload(attachment) => &attachment.owner,
            Self::HostImage { owner, .. } | Self::ToolImage { owner, .. } => owner,
        }
    }
}

impl From<Attachment> for AttachmentSource {
    fn from(attachment: Attachment) -> Self {
        Self::Upload(attachment)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentFile {
    pub name: String,
    pub media_type: String,
    pub source: AttachmentFileSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentFileSource {
    Original,
    HostRecord,
}

/// The streaming IPC upload uses raw frames after its Call, terminated by
/// an empty frame. This is a chunk size, not a file-size limit, and equals
/// the network data message size so forwarding needs only one chunk.
pub const UPLOAD_CHUNK: usize = 1 << 20;

/// What the machine an upload goes to says first over its data connection
/// when the call said it `resumes`: how many of the file's bytes it already
/// holds from an earlier try of the same upload, which the sender skips.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadStart {
    pub from: u64,
}
