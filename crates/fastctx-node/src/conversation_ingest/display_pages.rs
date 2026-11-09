//! Bounded descriptor upload plans. Publish body blocks first, then each payload
//! and its typed Metadata page. No ordinary read of private staged bytes is used.
use fastctx_types::sync::{
    PAGE_BYTES,
    conversation_timeline::{DisplayPartsRef, ItemDisplayPartsV1, ItemDisplayV1},
    cursor::Count,
    identity::{Digest, ScopeKey},
    manifest::{
        BlobId, ContentBlock, ContentRef, DigestAlgorithm, ManifestChild, ManifestPage, ManifestReply, ManifestRoot,
    },
    metadata::MetadataCodec,
    transport_codec::bounded_json,
};
use std::io;

pub struct DescriptorPage {
    scope: ScopeKey,
    codec: MetadataCodec,
    payload: Vec<u8>,
    references: Vec<ContentRef>,
    range: Option<(Count, Count)>,
}
impl DescriptorPage {
    /// The body writer verifies each Manifest part's decoded extent before this
    /// call. Single extents and descriptor/preview bounds are checked by the type.
    pub fn display(scope: ScopeKey, value: &ItemDisplayV1) -> io::Result<Self> {
        value.validate_scopes(&scope).map_err(io::Error::other)?;
        Ok(Self {
            scope,
            codec: MetadataCodec::DisplayV1,
            payload: value.encode().map_err(io::Error::other)?,
            references: value.owned_content(),
            range: None,
        })
    }
    pub fn parts(scope: ScopeKey, value: &ItemDisplayPartsV1) -> io::Result<Self> {
        value.validate_scopes(&scope).map_err(io::Error::other)?;
        Ok(Self {
            scope,
            codec: MetadataCodec::DisplayPartsV1,
            payload: value.encode().map_err(io::Error::other)?,
            references: value.owned_content(),
            range: Some((value.start, Count(value.parts.len() as u64))),
        })
    }
    /// Derives the range from the validated leaf, rather than a separately supplied
    /// count. The resulting reference can be passed to join_parts after upload.
    pub fn parts_manifest(&self, payload: ContentBlock) -> io::Result<(ManifestReply, DisplayPartsRef)> {
        let (start, count) = self.range.ok_or_else(|| io::Error::other("display root is not a parts leaf"))?;
        let page = self.manifest(payload)?;
        let reference = DisplayPartsRef { root: page.root.clone(), start, count };
        Ok((page, reference))
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// The returned payload block must be the exact bytes prepared here, uploaded
    /// through the same source step as the resulting manifest.
    pub fn manifest(&self, payload: ContentBlock) -> io::Result<ManifestReply> {
        if payload.id.domain != self.scope
            || payload.id.digest != Digest::of(&self.payload)
            || payload.decoded_bytes as usize != self.payload.len()
        {
            return Err(io::Error::other("descriptor upload differs from its prepared payload"));
        }
        page(&self.scope, &ManifestPage::Metadata { codec: self.codec, payload, references: self.references.clone() })
    }
}

pub struct PartsTree {
    pub reference: DisplayPartsRef,
    /// None for a single already-published child. Otherwise upload this Branch
    /// only after every child has been published through the same source step.
    pub page: Option<ManifestReply>,
}
/// Joins a bounded run of contiguous descriptor ranges. Larger trees use this
/// same operation level by level; no vector of every part is needed at the root.
pub fn join_parts(scope: &ScopeKey, children: &[DisplayPartsRef]) -> io::Result<PartsTree> {
    if children.is_empty() || children.len() > 256 {
        return Err(io::Error::other("descriptor branch needs 1 to 256 children"));
    }
    let start = children[0].start;
    let mut end = start.0;
    let mut pages = Vec::with_capacity(children.len());
    for child in children {
        if &child.root.0.domain != scope || child.start.0 != end || child.count.0 == 0 {
            return Err(io::Error::other("descriptor children cross scope or leave an index gap"));
        }
        end = end.checked_add(child.count.0).ok_or_else(|| io::Error::other("part range overflow"))?;
        pages.push(ManifestChild {
            first_key: format!("{:016x}", child.start.0),
            last_key: format!("{:016x}", end - 1),
            root: child.root.clone(),
            entries: child.count,
        });
    }
    if children.len() == 1 {
        return Ok(PartsTree { reference: children[0].clone(), page: None });
    }
    let page = page(scope, &ManifestPage::Branch { children: pages })?;
    Ok(PartsTree {
        reference: DisplayPartsRef { root: page.root.clone(), start, count: Count(end - start.0) },
        page: Some(page),
    })
}
fn page(scope: &ScopeKey, value: &ManifestPage) -> io::Result<ManifestReply> {
    let bytes = bounded_json(value, PAGE_BYTES as usize).map_err(io::Error::other)?;
    let root =
        ManifestRoot(BlobId { domain: scope.clone(), algorithm: DigestAlgorithm::Sha256, digest: Digest::of(&bytes) });
    Ok(ManifestReply { root, page_json: String::from_utf8(bytes).map_err(io::Error::other)? })
}
