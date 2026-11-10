//! Persistent byte trees with bounded pages and copy-on-write append paths.
mod append;
mod read;

use crate::{
    cursor::{ByteCount, Count},
    identity::Digest,
    manifest::{
        BlobId, ContentBlock, ContentRef, DigestAlgorithm, ManifestChild, ManifestPage, ManifestReply, ManifestRoot,
    },
    store::{self, FutureResult},
};
pub use append::append_block;
pub use read::{locate, read_range, read_range_at};
use serde::{Deserialize, Serialize};

pub const FANOUT: usize = 32;
pub const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyExtent {
    pub content: ContentRef,
    pub bytes: ByteCount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyTreeError {
    InvalidInput(String),
    CorruptContent(String),
    Store(store::Error),
    Budget { needed: usize, available: usize },
    CounterExhausted,
}
impl std::fmt::Display for BodyTreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "body tree: {self:?}")
    }
}
impl std::error::Error for BodyTreeError {}
impl From<store::Error> for BodyTreeError {
    fn from(error: store::Error) -> Self {
        Self::Store(error)
    }
}

/// IO must retain newly published pages for reads in the same uncommitted step.
/// Checkpoint readers use revision-bound source reads; public bodies use object reads.
pub trait BodyTreeIo: Send + Sync {
    fn read_manifest(&self, root: ManifestRoot) -> FutureResult<'_, ManifestReply>;
    fn read_block(&self, block: ContentBlock) -> FutureResult<'_, Vec<u8>>;
    fn put_manifest(&self, page: ManifestReply) -> FutureResult<'_, ()>;
    fn put_block(&self, bytes: Vec<u8>) -> FutureResult<'_, ContentBlock>;
}

type Result<T> = std::result::Result<T, BodyTreeError>;
fn corrupt(reason: &str) -> BodyTreeError {
    BodyTreeError::CorruptContent(reason.into())
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or(BodyTreeError::CounterExhausted)
}
fn key(offset: u64) -> String {
    format!("{offset:020}")
}
fn range(child: &ManifestChild) -> Result<(u64, u64)> {
    let first = child.first_key.parse::<u64>().map_err(|_| corrupt("invalid byte range"))?;
    let last = child.last_key.parse::<u64>().map_err(|_| corrupt("invalid byte range"))?;
    if key(first) != child.first_key || key(last) != child.last_key || first > last || child.entries.0 == 0 {
        return Err(corrupt("noncanonical byte range or count"));
    }
    Ok((first, add(last, 1)?))
}
fn children_extent(children: &[ManifestChild]) -> Result<(u64, u64, u64)> {
    if children.is_empty() || children.len() > FANOUT {
        return Err(corrupt("invalid branch fanout"));
    }
    let mut first = None;
    let mut end = None;
    let mut entries = 0;
    for child in children {
        let (start, through) = range(child)?;
        if end.is_some_and(|previous| previous != start) {
            return Err(corrupt("discontinuous branch"));
        }
        first.get_or_insert(start);
        end = Some(through);
        entries = add(entries, child.entries.0)?;
    }
    Ok((first.ok_or_else(|| corrupt("empty branch"))?, end.ok_or_else(|| corrupt("empty branch"))?, entries))
}
fn blocks_extent(offset: ByteCount, blocks: &[ContentBlock]) -> Result<(u64, u64, u64)> {
    if blocks.is_empty() || blocks.len() > FANOUT {
        return Err(corrupt("invalid leaf fanout"));
    }
    let mut end = offset.0;
    for block in blocks {
        if block.decoded_bytes == 0 || block.decoded_bytes > fastctx_types::sync::BLOCK_BYTES {
            return Err(corrupt("invalid leaf block length"));
        }
        end = add(end, u64::from(block.decoded_bytes))?;
    }
    Ok((offset.0, end, blocks.len() as u64))
}
async fn page(io: &dyn BodyTreeIo, root: ManifestRoot) -> Result<ManifestPage> {
    let reply = io.read_manifest(root.clone()).await?;
    if reply.root != root {
        return Err(corrupt("reader changed manifest identity"));
    }
    crate::blocks::manifest::validate(&root.0.domain, reply.clone()).map_err(|e| corrupt(&e.to_string()))?;
    serde_json::from_str(&reply.page_json).map_err(|e| corrupt(&e.to_string()))
}
fn extent(page: &ManifestPage) -> Result<(u64, u64, u64)> {
    match page {
        ManifestPage::Blocks { offset, blocks } => blocks_extent(*offset, blocks),
        ManifestPage::Branch { children } => children_extent(children),
        _ => Err(corrupt("expected a byte-tree page")),
    }
}
/// The validated origin and logical length of one complete byte-tree subtree.
pub fn page_extent(page: &ManifestPage) -> Result<(ByteCount, ByteCount)> {
    let (start, end, _) = extent(page)?;
    Ok((ByteCount(start), ByteCount(end.checked_sub(start).ok_or_else(|| corrupt("byte-tree extent is reversed"))?)))
}
fn verify_child(child: &ManifestChild, page: &ManifestPage) -> Result<()> {
    let (start, end, entries) = extent(page)?;
    if range(child)? != (start, end) || child.entries.0 != entries {
        return Err(corrupt("child extent or count differs"));
    }
    Ok(())
}
async fn publish(io: &dyn BodyTreeIo, scope: &crate::identity::ScopeKey, page: ManifestPage) -> Result<ManifestChild> {
    let (first, end, entries) = extent(&page)?;
    let page_json = serde_json::to_string(&page).map_err(|e| corrupt(&e.to_string()))?;
    let root = ManifestRoot(BlobId {
        domain: scope.clone(),
        algorithm: DigestAlgorithm::Sha256,
        digest: Digest::of(page_json.as_bytes()),
    });
    let reply = ManifestReply { root: root.clone(), page_json };
    crate::blocks::manifest::validate(scope, reply.clone()).map_err(|e| corrupt(&e.to_string()))?;
    io.put_manifest(reply).await?;
    Ok(ManifestChild { first_key: key(first), last_key: key(end - 1), root, entries: Count(entries) })
}
