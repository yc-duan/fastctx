//! Page-local correspondence; no reverse edges or general subtree search.
mod address;
mod bytes;

use crate::{
    identity::Digest,
    manifest::{ContentRef, DirectoryEntry, ManifestPage, ManifestReply},
    store::sql,
};
use address::Part;
pub(super) use address::{Address, Span};
use fastctx_storage::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Child {
    pub root: ContentRef,
    pub address: Address,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Layout {
    pub address: Address,
    pub children: Vec<Child>,
}
impl Layout {
    pub fn single(root: &ContentRef, address: &Address) -> Result<Self> {
        let ContentRef::Single { decoded_bytes, .. } = root else {
            return Err(sql::invalid("a manifest needs its verified page"));
        };
        let start = address.span.map_or(0, |span| span.start);
        let end =
            start.checked_add(u64::from(*decoded_bytes)).ok_or_else(|| sql::invalid("block interval overflow"))?;
        Ok(Self { address: address.resolve(Some(Span::new(start, end)?))?, children: Vec::new() })
    }

    pub fn page(reply: &ManifestReply, address: &Address) -> Result<Self> {
        if reply.page_json.len() > fastctx_types::sync::PAGE_BYTES as usize
            || Digest::of(reply.page_json.as_bytes()) != reply.root.0.digest
        {
            return Err(sql::invalid("supply manifest differs from its bounded digest"));
        }
        let page: ManifestPage = serde_json::from_str(&reply.page_json).map_err(|e| sql::invalid(e.to_string()))?;
        let local = Address::default();
        let (extent, children) = match page {
            ManifestPage::Blocks { offset, blocks } => {
                let (span, children) = bytes::blocks(&local, offset.0, blocks)?;
                (Some(span), children)
            }
            ManifestPage::Branch { children } => {
                if let Some(ranges) = bytes::intervals(&children) {
                    let (span, children) = bytes::branch(&local, children, ranges)?;
                    (Some(span), children)
                } else {
                    let children = children
                        .into_iter()
                        .map(|child| {
                            Ok(Child {
                                root: ContentRef::Manifest { root: child.root },
                                address: local
                                    .structural(Part::Range { first: child.first_key, last: child.last_key })?,
                            })
                        })
                        .collect::<Result<_>>()?;
                    (None, children)
                }
            }
            ManifestPage::Directory { entries } => {
                let mut children = Vec::new();
                for entry in entries {
                    let (path, root) = match entry {
                        DirectoryEntry::File { file } => (file.path, file.content),
                        DirectoryEntry::Directory { path, root: Some(root) } => (path, ContentRef::Manifest { root }),
                        DirectoryEntry::Directory { root: None, .. } | DirectoryEntry::Link { .. } => continue,
                    };
                    children.push(Child { root, address: local.path(&path.components)? });
                }
                (None, children)
            }
            ManifestPage::Metadata { codec, payload, references } => {
                let codec = serde_json::to_string(&codec).map_err(|e| sql::invalid(e.to_string()))?;
                let mut children = Vec::with_capacity(references.len() + 1);
                for (index, root) in std::iter::once(ContentRef::from(payload)).chain(references).enumerate() {
                    let index = u32::try_from(index).map_err(|_| sql::invalid("too many index reference positions"))?;
                    children.push(Child {
                        root,
                        address: local.structural(Part::Metadata { codec: codec.clone(), index })?,
                    });
                }
                (None, children)
            }
        };
        if children.len() > 4096 || children.iter().any(|child| child.root.id().domain != reply.root.0.domain) {
            return Err(sql::invalid("supply page children exceed their scope or count bound"));
        }
        Ok(Self { address: address.resolve(extent)?, children })
    }

    /// A basis may descend only into the unique child containing the requested
    /// logical position. A new page spanning several old children must itself be
    /// supplied; its children will each be checked against the same declared basis.
    pub fn toward(&self, target: &Address) -> Result<Option<Child>> {
        let mut found: Option<Child> = None;
        for index in 0..self.children.len() {
            let child = self.child(index)?.expect("existing child index");
            if !child.address.contains(target) {
                continue;
            }
            if let Some(previous) = &found {
                if previous != &child {
                    return Err(sql::invalid("basis page has ambiguous containing positions"));
                }
            } else {
                found = Some(child);
            }
        }
        Ok(found)
    }
    pub fn child(&self, index: usize) -> Result<Option<Child>> {
        self.children
            .get(index)
            .map(|child| Ok(Child { root: child.root.clone(), address: self.address.join(&child.address)? }))
            .transpose()
    }
}
