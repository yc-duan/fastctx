//! Content-addressed blocks and bounded pages of file and directory manifests.

use super::{
    cursor::ByteCount,
    identity::{Digest, ObjectId, ScopeKey},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlobId {
    /// Authorization is attached to this domain; knowing the digest grants none.
    pub domain: ScopeKey,
    pub algorithm: DigestAlgorithm,
    pub digest: Digest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithm {
    Sha256,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Raw,
    Zstd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protection {
    Plain,
}

/// The first version uses plaintext. Future protected representations require
/// explicit capability negotiation rather than reinterpreting existing bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u16,
    pub encoding: Encoding,
    pub protection: Protection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// Digest of the decoded representation in the current plaintext version.
    pub id: BlobId,
    pub decoded_bytes: u32,
    pub encoded_bytes: u32,
    pub encoded_digest: Digest,
    pub envelope: Envelope,
}

impl Block {
    pub fn validate(&self) -> Result<(), String> {
        if self.decoded_bytes > super::BLOCK_BYTES || self.encoded_bytes > super::BLOCK_BYTES + 64 * 1024 {
            return Err("a block exceeds the negotiated storage bound".into());
        }
        if self.envelope.version != 1 {
            return Err("unsupported block envelope".into());
        }
        Ok(())
    }
}

/// Names are preserved even when the receiving filesystem needs a reversible
/// spelling. Importers reject separators, traversal and NUL in each component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
pub enum PathName {
    Utf8(String),
    UnixBytes(Vec<u8>),
    WindowsUnits(Vec<u16>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelativePath {
    pub components: Vec<PathName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub object: ObjectId,
    pub path: RelativePath,
    pub bytes: ByteCount,
    pub executable: bool,
    pub content: BlobId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DirectoryEntry {
    File {
        file: FileEntry,
    },
    Directory {
        path: RelativePath,
        root: Option<BlobId>,
    },
    /// A link is data, never permission to read its target while importing.
    Link {
        path: RelativePath,
        target: PathName,
    },
}

/// Serialized pages are bounded by PAGE_BYTES. Branches permit arbitrary asset
/// size without retaining its complete block list in a message or a process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManifestPage {
    Blocks { offset: ByteCount, blocks: Vec<Block> },
    Directory { entries: Vec<DirectoryEntry> },
    Branch { children: Vec<ManifestChild> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestChild {
    pub first_key: String,
    pub last_key: String,
    pub root: BlobId,
    pub entries: ByteCount,
}
