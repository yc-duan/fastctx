//! ManifestV1 is selected by the publication entry point, never content sniffing.

use crate::{
    cursor::{Holder, HoldingPeriod, Revision},
    identity::{DeviceId, Digest, ScopeKey},
    manifest::{BlobId, DirectoryEntry, FileContent, ManifestPage, ManifestReply, PathName, RelativePath},
    store::Error,
    transfer::Message,
};
use fastctx_types::sync::{BLOCK_BYTES, PAGE_BYTES};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reference {
    Block { id: BlobId, decoded_bytes: u32 },
    Manifest { root: BlobId },
}
impl Reference {
    pub fn id(&self) -> &BlobId {
        match self {
            Self::Block { id, .. } => id,
            Self::Manifest { root } => root,
        }
    }
}

pub struct CheckedManifest {
    pub(crate) reply: ManifestReply,
    pub(crate) references: Vec<Reference>,
}
impl CheckedManifest {
    pub fn reply(&self) -> &ManifestReply {
        &self.reply
    }
    pub fn references(&self) -> &[Reference] {
        &self.references
    }
}

/// Validates the exact page and its registered references without touching disk.
/// Local producers submit their encoded page through this same codec.
pub fn validate(scope: &ScopeKey, reply: ManifestReply) -> Result<CheckedManifest, Error> {
    if reply.page_json.len() > PAGE_BYTES as usize {
        return Err(Error::Invalid("manifest page exceeds its metadata budget".into()));
    }
    if &reply.root.domain != scope || Digest::of(reply.page_json.as_bytes()) != reply.root.digest {
        return Err(Error::Invalid("manifest bytes differ from the scoped content identity".into()));
    }
    // Count JSON escaping and the longest V1 holding fields, not only raw JSON.
    let holding = HoldingPeriod {
        holder: Holder::Device { device: DeviceId::from_bytes([0; 32]) },
        scope: scope.clone(),
        acl_revision: Revision(u64::MAX),
        publication_epoch: Revision(u64::MAX),
    };
    let frame = Message::Manifest { holding, reply: reply.clone() };
    if serde_json::to_vec(&frame).map_err(invalid)?.len() > PAGE_BYTES as usize {
        return Err(Error::Invalid("escaped manifest page exceeds its wire budget".into()));
    }
    let page: ManifestPage = serde_json::from_str(&reply.page_json).map_err(invalid)?;
    let mut references = BTreeMap::new();
    match page {
        ManifestPage::Blocks { offset, blocks } => {
            let mut through = offset.0;
            for block in blocks {
                if block.decoded_bytes > BLOCK_BYTES {
                    return Err(Error::Invalid("manifest block exceeds one MiB".into()));
                }
                through = through
                    .checked_add(u64::from(block.decoded_bytes))
                    .ok_or_else(|| Error::Invalid("manifest extent overflows its byte cursor".into()))?;
                add(&mut references, Reference::Block { id: block.id, decoded_bytes: block.decoded_bytes })?;
            }
        }
        ManifestPage::Directory { entries } => {
            let mut paths = BTreeSet::new();
            let mut objects = BTreeSet::new();
            for entry in entries {
                let path = match &entry {
                    DirectoryEntry::File { file } => &file.path,
                    DirectoryEntry::Directory { path, .. } | DirectoryEntry::Link { path, .. } => path,
                };
                if !paths.insert(path_key(path)?) {
                    return Err(Error::Invalid("a directory page repeats a path".into()));
                }
                match entry {
                    DirectoryEntry::File { file } => {
                        if !objects.insert(file.object) {
                            return Err(Error::Invalid("a directory page repeats a file identity".into()));
                        }
                        match file.content {
                            FileContent::Single { block, decoded_bytes } => {
                                if decoded_bytes > BLOCK_BYTES || file.bytes.0 != u64::from(decoded_bytes) {
                                    return Err(Error::Invalid(
                                        "single-block file length differs from its entry".into(),
                                    ));
                                }
                                add(&mut references, Reference::Block { id: block, decoded_bytes })?;
                            }
                            FileContent::Manifest { root } => add(&mut references, Reference::Manifest { root })?,
                        }
                    }
                    DirectoryEntry::Directory { root: Some(root), .. } => {
                        add(&mut references, Reference::Manifest { root })?;
                    }
                    DirectoryEntry::Directory { root: None, .. } | DirectoryEntry::Link { .. } => {}
                }
            }
        }
        ManifestPage::Branch { children } => {
            let mut previous = None;
            for child in children {
                if child.first_key > child.last_key || previous.as_ref().is_some_and(|last| last >= &child.first_key) {
                    return Err(Error::Invalid("manifest child ranges overlap or run backwards".into()));
                }
                previous = Some(child.last_key);
                add(&mut references, Reference::Manifest { root: child.root })?;
            }
        }
    }
    if references.values().any(|reference| &reference.id().domain != scope || reference.id() == &reply.root) {
        return Err(Error::Invalid("manifest references must stay in their scope and exclude the page itself".into()));
    }
    Ok(CheckedManifest { reply, references: references.into_values().collect() })
}

fn add(references: &mut BTreeMap<(BlobId, bool), Reference>, reference: Reference) -> Result<(), Error> {
    let key = (reference.id().clone(), matches!(reference, Reference::Manifest { .. }));
    if let Some(previous) = references.insert(key, reference.clone())
        && previous != reference
    {
        return Err(Error::Invalid("the same content block has conflicting lengths".into()));
    }
    Ok(())
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum NameKey {
    Text(String),
    Unix(Vec<u8>),
    Windows(Vec<u16>),
}

fn path_key(path: &RelativePath) -> Result<Vec<NameKey>, Error> {
    if path.components.is_empty() {
        return Err(Error::Invalid("a manifest entry needs a relative path".into()));
    }
    path.components
        .iter()
        .map(|name| match name {
            PathName::Utf8(text) => {
                if text.is_empty()
                    || matches!(text.as_str(), "." | "..")
                    || text.contains(['\0', '/', '\\'])
                    || (text.as_bytes().get(1) == Some(&b':') && text.as_bytes()[0].is_ascii_alphabetic())
                {
                    return Err(Error::Invalid("invalid portable manifest path component".into()));
                }
                Ok(NameKey::Text(text.clone()))
            }
            PathName::UnixBytes(bytes) => {
                if bytes.is_empty()
                    || bytes.contains(&0)
                    || bytes.contains(&b'/')
                    || matches!(bytes.as_slice(), b"." | b"..")
                {
                    return Err(Error::Invalid("invalid Unix manifest path component".into()));
                }
                Ok(String::from_utf8(bytes.clone()).map(NameKey::Text).unwrap_or_else(|_| NameKey::Unix(bytes.clone())))
            }
            PathName::WindowsUnits(units) => {
                if units.is_empty()
                    || units.iter().any(|unit| matches!(*unit, 0 | 47 | 92 | 58))
                    || matches!(units.as_slice(), [46] | [46, 46])
                {
                    return Err(Error::Invalid("invalid Windows manifest path component".into()));
                }
                Ok(String::from_utf16(units).map(NameKey::Text).unwrap_or_else(|_| NameKey::Windows(units.clone())))
            }
        })
        .collect()
}

fn invalid(error: serde_json::Error) -> Error {
    Error::Invalid(format!("invalid ManifestV1 page: {error}"))
}
