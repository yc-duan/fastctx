//! A witness starts at this fenced item's exact roots and proves each direct edge.
use super::Io;
use crate::{
    identity::ScopeKey,
    manifest::{ContentRef, DirectoryEntry, ManifestPage},
    store::Error,
    timeline::TimelineItem,
};
use fastctx_types::sync::timeline::CONTENT_PATH_DEPTH;
use std::collections::BTreeSet;

pub(super) fn validate_path(scope: &ScopeKey, target: &ContentRef, path: &[ContentRef]) -> Result<(), Error> {
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
    path: &[ContentRef],
) -> Result<bool, Error> {
    let mut roots = item.display.owned_content();
    roots.push(item.body.clone());
    let Some(mut parent) = path.first() else {
        return Ok(roots.iter().any(|root| root == target));
    };
    if !roots.iter().any(|root| root == parent) {
        return Ok(false);
    }
    for child in path.iter().skip(1).chain(std::iter::once(target)) {
        if !children(io, parent)?.iter().any(|reference| reference == child) {
            return Ok(false);
        }
        parent = child;
    }
    Ok(true)
}
fn children(io: &Io<'_>, content: &ContentRef) -> Result<Vec<ContentRef>, Error> {
    let ContentRef::Manifest { root } = content else { return Ok(Vec::new()) };
    let page = io.page(root)?;
    if matches!(page, ManifestPage::Metadata { .. }) {
        io.validate_metadata(root, &page)?;
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
