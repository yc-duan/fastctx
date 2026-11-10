//! Reachability starts at exactly one fenced item's owned roots, never at the
//! conversation-wide grant set or at a client-supplied manifest.
use super::Io;
use crate::{
    manifest::{ContentRef, DirectoryEntry, ManifestPage},
    store::Error,
    timeline::TimelineItem,
};
use std::collections::BTreeMap;

pub(super) fn contains(io: &Io<'_>, item: &TimelineItem, target: &ContentRef) -> Result<bool, Error> {
    let mut roots = item.display.owned_content();
    roots.push(item.body.clone());
    if roots.iter().any(|root| root == target) {
        return Ok(true);
    }
    let mut seen = BTreeMap::new();
    while let Some(content) = roots.pop() {
        if seen.len() >= 65_536 {
            return Err(Error::Busy);
        }
        io.permits.lock().expect("never poisoned").push(io.blocks.buffer_budget().try_acquire(512)?);
        let key = (content.id().clone(), matches!(content, ContentRef::Manifest { .. }));
        if let Some(previous) = seen.insert(key, content.clone()) {
            if previous != content {
                return Err(Error::Corrupt("owned closure has conflicting content lengths".into()));
            }
            continue;
        }
        let ContentRef::Manifest { root } = content else {
            continue;
        };
        let page = io.page(&root)?;
        // Metadata references carry authority only after the registered payload
        // has been validated. Missing payloads remain Pending, never Denied.
        io.metadata(&root, &page)?;
        let children = match page {
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
        };
        if children.iter().any(|child| child == target) {
            return Ok(true);
        }
        roots.extend(children);
    }
    Ok(false)
}
