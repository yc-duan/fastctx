//! Files the user gives a resident, stored on the machine running its host.

use serde::{Deserialize, Serialize};

use crate::{Address, AgentName, NoticeId, ResidentId, TownName};

/// Where an upload belongs on the receiving machine. A new conversation
/// reserves its resident before any host starts; subsequent files use Draft.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UploadTarget {
    Resident { resident: ResidentId },
    New { agent: AgentName, town: Option<TownName> },
    Draft { resident: ResidentId, town: Option<TownName> },
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

impl Attachment {
    pub fn is_image(&self) -> bool {
        self.file.media_type.starts_with("image/")
    }
}

/// The streaming IPC upload uses raw frames after its Call, terminated by
/// an empty frame. This is a chunk size, not a file-size limit, and equals
/// the network data message size so forwarding needs only one chunk.
pub const UPLOAD_CHUNK: usize = 1 << 20;
