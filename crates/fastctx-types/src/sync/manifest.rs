//! Manifests identify decoded content. Compression belongs to representations.

use super::{
    cursor::{ByteCount, Count},
    identity::{Digest, ObjectId, ScopeKey},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlobId {
    pub domain: ScopeKey,
    pub algorithm: DigestAlgorithm,
    pub digest: Digest,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithm {
    Sha256,
}

/// Strict values are used only after selecting a mutually advertised encoding.
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u16,
    pub encoding: Encoding,
    pub protection: Protection,
}

/// A manifest's block reference makes no promise about a compressed form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentBlock {
    pub id: BlobId,
    pub decoded_bytes: u32,
}

/// Multiple representations may coexist for one BlobId. The encoded digest
/// identifies selected transfer bytes, and never changes the content identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Representation {
    pub content: ContentBlock,
    pub encoded_bytes: u32,
    pub encoded_digest: Digest,
    pub envelope: Envelope,
}
impl Representation {
    /// Identifies an encoding choice without changing the decoded BlobId.
    pub fn id(&self) -> Digest {
        let mut hash = Sha256::new();
        hash.update(b"fastctx.sync.representation.v1\0");
        hash.update(self.envelope.version.to_be_bytes());
        hash.update([match self.envelope.encoding {
            Encoding::Raw => 0,
            Encoding::Zstd => 1,
        }]);
        hash.update([match self.envelope.protection {
            Protection::Plain => 0,
        }]);
        hash.update(self.encoded_bytes.to_be_bytes());
        hash.update(self.encoded_digest.as_str().as_bytes());
        Digest::from_bytes(hash.finalize().into())
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.content.decoded_bytes > super::BLOCK_BYTES || self.encoded_bytes > super::BLOCK_BYTES + 64 * 1024 {
            return Err("a representation exceeds the storage block bound".into());
        }
        if self.envelope.version != 1 {
            return Err("unsupported selected envelope".into());
        }
        Ok(())
    }
}

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
    pub content: ContentRef,
}

/// Variable-sized bodies directly reference a block or explicitly name a
/// validated manifest. JSON-shaped bytes never implicitly become a manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentRef {
    Single { block: BlobId, decoded_bytes: u32 },
    Manifest { root: ManifestRoot },
}
impl ContentRef {
    pub fn id(&self) -> &BlobId {
        match self {
            Self::Single { block, .. } => block,
            Self::Manifest { root } => &root.0,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if matches!(self, Self::Single { decoded_bytes, .. } if *decoded_bytes > super::BLOCK_BYTES) {
            return Err("a single content reference exceeds one MiB".into());
        }
        Ok(())
    }
}
impl From<ContentBlock> for ContentRef {
    fn from(block: ContentBlock) -> Self {
        Self::Single { block: block.id, decoded_bytes: block.decoded_bytes }
    }
}

/// A field of this type always requires a verified manifest closure. Its JSON
/// identity remains a BlobId; interpretation comes from the declared field.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ManifestRoot(pub BlobId);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DirectoryEntry {
    File { file: FileEntry },
    Directory { path: RelativePath, root: Option<ManifestRoot> },
    Link { path: RelativePath, target: PathName },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManifestPage {
    Blocks { offset: ByteCount, blocks: Vec<ContentBlock> },
    Directory { entries: Vec<DirectoryEntry> },
    Branch { children: Vec<ManifestChild> },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestChild {
    pub first_key: String,
    pub last_key: String,
    pub root: ManifestRoot,
    pub entries: Count,
}

/// Exact bounded JSON bytes are hashed before decoding ManifestPage. Readers
/// do not hash a reserialized value, and may follow an unfamiliar history root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestReply {
    pub root: ManifestRoot,
    pub page_json: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    pub content: ContentBlock,
    pub representations: Vec<Representation>,
}
