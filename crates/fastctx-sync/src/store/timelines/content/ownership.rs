//! A witness starts at this fenced item's exact roots and proves each direct edge.
use super::Io;
use crate::{
    identity::ScopeKey,
    manifest::{BlobId, ContentRef, DirectoryEntry, ManifestPage},
    store::Error,
    timeline::TimelineItem,
};
use fastctx_types::sync::timeline::CONTENT_PATH_DEPTH;
use std::collections::{BTreeMap, BTreeSet};

const LEGACY_READS: usize = 512;
const LEGACY_NODES: usize = 4096;
const NODE_BYTES: usize = 1024;

pub(super) fn validate_path(scope: &ScopeKey, target: &ContentRef, path: Option<&[ContentRef]>) -> Result<(), Error> {
    let Some(path) = path else { return Ok(()) };
    if path.len() > CONTENT_PATH_DEPTH {
        return Err(Error::Invalid("content ownership path exceeds 128 edges".into()));
    }
    let mut seen = BTreeSet::new();
    for content in path.iter().chain(std::iter::once(target)) {
        content.validate().map_err(Error::Invalid)?;
        if &content.id().domain != scope {
            return Err(Error::Denied("the ownership path belongs to another scope".into()));
        }
        if !seen.insert((content.id(), matches!(content, ContentRef::Manifest { .. }))) {
            return Err(Error::Invalid("content ownership path repeats a node".into()));
        }
    }
    Ok(())
}

pub(super) fn contains(
    io: &Io<'_>,
    item: &TimelineItem,
    target: &ContentRef,
    path: Option<&[ContentRef]>,
) -> Result<bool, Error> {
    let mut roots = item.display.owned_content();
    roots.push(item.body.clone());
    match path {
        Some(parents) => prove(io, &roots, target, parents),
        None if roots.iter().any(|root| root == target) => Ok(true),
        None => legacy(io, roots, target),
    }
}
fn prove(io: &Io<'_>, roots: &[ContentRef], target: &ContentRef, parents: &[ContentRef]) -> Result<bool, Error> {
    let Some(mut parent) = parents.first() else {
        return Ok(roots.iter().any(|root| root == target));
    };
    if !roots.iter().any(|root| root == parent) {
        return Ok(false);
    }
    let reader = Parents { io, start: io.reads(), limit: None };
    for child in parents.iter().skip(1).chain(std::iter::once(target)) {
        if !reader.children(parent)?.iter().any(|reference| reference == child) {
            return Ok(false);
        }
        parent = child;
    }
    Ok(true)
}
fn legacy(io: &Io<'_>, roots: Vec<ContentRef>, target: &ContentRef) -> Result<bool, Error> {
    let budget = io.blocks.buffer_budget();
    let node_limit = LEGACY_NODES.min(budget.capacity() / 4 / NODE_BYTES);
    let mut seen = BTreeMap::<(BlobId, bool), ContentRef>::new();
    let mut pending = Vec::new();
    let mut permits = Vec::new();
    let mut schedule = |content: ContentRef, depth: usize, pending: &mut Vec<_>| -> Result<(), Error> {
        let key = (content.id().clone(), matches!(content, ContentRef::Manifest { .. }));
        if let Some(previous) = seen.get(&key) {
            return if previous == &content {
                Ok(())
            } else {
                Err(Error::Corrupt("owned closure has conflicting content lengths".into()))
            };
        }
        if seen.len() >= node_limit {
            return Err(path_required());
        }
        permits.push(budget.try_acquire(NODE_BYTES)?);
        seen.insert(key, content.clone());
        pending.push((content, depth));
        Ok(())
    };
    for root in roots {
        schedule(root, 0, &mut pending)?;
    }
    let reader = Parents { io, start: io.reads(), limit: Some(LEGACY_READS) };
    while let Some((content, depth)) = pending.pop() {
        if matches!(content, ContentRef::Single { .. }) {
            continue;
        }
        if depth >= CONTENT_PATH_DEPTH {
            return Err(path_required());
        }
        let children = reader.children(&content)?;
        if children.iter().any(|child| child == target) {
            return Ok(true);
        }
        for child in children {
            schedule(child, depth + 1, &mut pending)?;
        }
    }
    Ok(false)
}
fn path_required() -> Error {
    Error::Invalid("content ownership path required; repeat the request with path".into())
}
struct Parents<'a, 'b> {
    io: &'a Io<'b>,
    start: usize,
    limit: Option<usize>,
}
impl Parents<'_, '_> {
    fn fetch(&self) -> Result<(), Error> {
        if self.limit.is_some_and(|limit| self.io.reads().saturating_sub(self.start) >= limit) {
            return Err(path_required());
        }
        Ok(())
    }
    fn children(&self, content: &ContentRef) -> Result<Vec<ContentRef>, Error> {
        let ContentRef::Manifest { root } = content else { return Ok(Vec::new()) };
        self.fetch()?;
        let page = self.io.page(root)?;
        if matches!(page, ManifestPage::Metadata { .. }) {
            self.fetch()?;
            self.io.validate_metadata(root, &page)?;
        }
        Ok(match page {
            ManifestPage::Blocks { blocks, .. } => blocks.into_iter().map(ContentRef::from).collect(),
            ManifestPage::Branch { children } => {
                children.into_iter().map(|child| ContentRef::Manifest { root: child.root }).collect()
            }
            ManifestPage::Metadata { payload, mut references, .. } => {
                references.push(ContentRef::from(payload));
                references
            }
            ManifestPage::Directory { entries } => entries
                .into_iter()
                .filter_map(|entry| match entry {
                    DirectoryEntry::File { file } => Some(file.content),
                    DirectoryEntry::Directory { root: Some(root), .. } => Some(ContentRef::Manifest { root }),
                    _ => None,
                })
                .collect(),
        })
    }
}
