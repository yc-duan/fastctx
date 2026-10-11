//! Byte reads for the user's interfaces. Paths belong to the named machine;
//! bytes are never decoded, rendered, or collected into a whole-file reply.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::MachineName;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadFileRequest {
    pub path: String,
    pub range: FileRange,
}

/// Ranges use the size observed when the file is opened. They stop at that
/// size even if it grows. An offset past EOF selects an empty range at EOF.
/// Zero bytes selects an empty range. No caller-selected length is capped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FileRange {
    All,
    Tail { bytes: u64 },
    Range { offset: u64, bytes: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMetadata {
    pub size: u64,
    pub modified: Timestamp,
}

/// The initial observation and the exact selected byte interval
/// `[offset, offset + length)`. This is not an atomic filesystem snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReadInfo {
    pub machine: MachineName,
    pub path: String,
    pub file: FileMetadata,
    pub offset: u64,
    pub length: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<crate::attachments::AttachmentFile>,
}

/// A successful read: all selected bytes arrived and the file's identity,
/// size, modification time and filesystem change time were unchanged at
/// the final check. Filesystems may not expose every concurrent write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReadComplete {
    pub bytes: u64,
    pub file: FileMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileReadErrorKind {
    NotFound,
    PermissionDenied,
    IsDirectory,
    NotRegularFile,
    InvalidInput,
    Changed,
    Io,
}

/// Bytes already delivered are provisional if this result is an error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct FileReadError {
    pub kind: FileReadErrorKind,
    pub message: String,
    pub bytes: u64,
    /// The final observation, when available (including on `changed`).
    pub observed: Option<FileMetadata>,
}

pub type FileReadResult = Result<FileReadComplete, FileReadError>;

/// Local IPC: `Opened`, then zero or more `Chunk` headers, then
/// `Reply::FileRead`. Each chunk header is immediately followed by one
/// length-prefixed **binary** IPC frame containing exactly `bytes` bytes.
/// A terminal reply is required even when the selected interval is empty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FileReadPart {
    Opened { info: FileReadInfo },
    Chunk { offset: u64, bytes: u64 },
}
