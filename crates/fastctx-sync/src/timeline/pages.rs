//! Pure page access over an admitted, authorized memory view. Missing data is
//! returned to the executor; these functions never fetch it or open a writer.
use crate::{
    blocks::{manifest, metadata},
    identity::{Digest, ScopeKey},
    manifest::{BlobId, ContentBlock, ContentRef, DigestAlgorithm, ManifestPage, ManifestReply, ManifestRoot},
    store::Error,
};
use fastctx_types::sync::{
    PAGE_BYTES,
    metadata::{MetadataCodec, TimelinePayload},
    transport_codec::bounded_json,
};
use std::collections::BTreeMap;

/// An authorized, admitted memory view for one planning or read operation.
/// Implementations return exact already-available bytes, never perform IO from
/// these methods, and keep returned slices stable for the duration of the call.
pub trait PageSource {
    fn manifest(&self, root: &ManifestRoot) -> Result<Option<&[u8]>, Error>;
    fn payload(&self, block: &ContentBlock) -> Result<Option<&[u8]>, Error>;
}
#[derive(Debug)]
/// Pending identifies the first missing dependency. The executor fetches it and
/// retries against the same fenced input; no partial output has been published.
pub enum Outcome<T> {
    Ready(T),
    Pending { missing: ContentRef },
}
#[derive(Clone, Copy, Debug)]
/// Conservative cumulative preparation work, including repeated decoding and
/// intermediate copy-on-write pages. It is not a measured resident-heap limit.
/// Executors admit this allowance and reduce a batch when it returns Busy.
pub struct Budget {
    pub work_bytes: usize,
    pub max_pages: usize,
    pub max_changes: usize,
}
#[derive(Clone, Debug)]
pub struct EncodedPage {
    pub manifest: ManifestReply,
    pub payload: Option<(ContentBlock, Vec<u8>)>,
}
#[derive(Debug)]
pub(super) enum Fault {
    Error(Error),
    Missing(ContentRef),
}
impl From<Error> for Fault {
    fn from(error: Error) -> Self {
        Self::Error(error)
    }
}
pub(super) type Work<T> = std::result::Result<T, Fault>;
pub(super) fn outcome<T>(value: Work<T>) -> std::result::Result<Outcome<T>, Error> {
    match value {
        Ok(v) => Ok(Outcome::Ready(v)),
        Err(Fault::Missing(missing)) => Ok(Outcome::Pending { missing }),
        Err(Fault::Error(e)) => Err(e),
    }
}
pub(super) struct Meter {
    pub budget: Budget,
    bytes: usize,
    pages: usize,
}
impl Meter {
    pub fn pages(&self) -> usize {
        self.pages
    }
    pub fn new(budget: Budget) -> Self {
        Self { budget, bytes: 0, pages: 0 }
    }
    pub fn charge(&mut self, bytes: usize) -> std::result::Result<(), Error> {
        self.bytes = self.bytes.checked_add(bytes).ok_or(Error::Busy)?;
        if self.bytes > self.budget.work_bytes {
            return Err(Error::Busy);
        }
        Ok(())
    }
    pub fn page(&mut self, bytes: usize) -> std::result::Result<(), Error> {
        self.pages = self.pages.checked_add(1).ok_or(Error::Busy)?;
        if self.pages > self.budget.max_pages {
            return Err(Error::Busy);
        }
        self.charge(bytes.checked_mul(8).ok_or(Error::Busy)?)
    }
}
pub(super) enum Node {
    Branch(Vec<crate::manifest::ManifestChild>),
    Payload(TimelinePayload),
}
pub(super) fn load(source: &impl PageSource, root: &ManifestRoot, meter: &mut Meter) -> Work<Node> {
    let bytes = source.manifest(root)?.ok_or_else(|| Fault::Missing(ContentRef::Manifest { root: root.clone() }))?;
    meter.page(bytes.len())?;
    let page_json = std::str::from_utf8(bytes).map_err(|_| corrupt("manifest is not UTF-8"))?;
    manifest::validate(&root.0.domain, ManifestReply { root: root.clone(), page_json: page_json.into() })
        .map_err(as_corrupt)?;
    let page: ManifestPage = serde_json::from_slice(bytes).map_err(|_| corrupt("invalid timeline manifest"))?;
    match page {
        ManifestPage::Branch { children } => {
            if children.is_empty() || children.len() > 256 || children.iter().any(|c| c.entries.0 == 0) {
                return Err(corrupt("invalid timeline branch").into());
            }
            Ok(Node::Branch(children))
        }
        ManifestPage::Metadata { codec: MetadataCodec::TimelineV1, payload, references } => {
            let bytes = source.payload(&payload)?.ok_or_else(|| Fault::Missing(ContentRef::from(payload.clone())))?;
            meter.charge(bytes.len().checked_mul(8).ok_or(Error::Busy)?)?;
            metadata::validate(&root.0.domain, MetadataCodec::TimelineV1, &payload, bytes, &references)
                .map_err(as_corrupt)?;
            Ok(Node::Payload(serde_json::from_slice(bytes).map_err(|_| corrupt("invalid timeline payload"))?))
        }
        _ => Err(corrupt("not a TimelineV1 tree page").into()),
    }
}
pub(super) fn encode_payload(
    scope: &ScopeKey,
    value: &TimelinePayload,
) -> std::result::Result<Option<EncodedPage>, Error> {
    let references = metadata::timeline_references(scope, value)?;
    let references = unique(references)?;
    let Ok(bytes) = bounded_json(value, PAGE_BYTES as usize) else { return Ok(None) };
    let payload = ContentBlock { id: blob(scope, &bytes), decoded_bytes: bytes.len() as u32 };
    let manifest = ManifestPage::Metadata { codec: MetadataCodec::TimelineV1, payload: payload.clone(), references };
    let Some(reply) = encode_manifest(scope, &manifest)? else { return Ok(None) };
    // Proof references are included in the payload and envelope sizes. Exact
    // proof wrappers are separate bounded blocks, shared by all referring pages.
    if bytes.len().checked_add(wire_bytes(&reply)?).is_none_or(|n| n > PAGE_BYTES as usize) {
        return Ok(None);
    }
    metadata::validate(
        scope,
        MetadataCodec::TimelineV1,
        &payload,
        &bytes,
        match &manifest {
            ManifestPage::Metadata { references, .. } => references,
            _ => unreachable!(),
        },
    )?;
    Ok(Some(EncodedPage { manifest: reply, payload: Some((payload, bytes)) }))
}
pub(super) fn encode_branch(
    scope: &ScopeKey,
    children: Vec<crate::manifest::ManifestChild>,
) -> std::result::Result<Option<EncodedPage>, Error> {
    Ok(encode_manifest(scope, &ManifestPage::Branch { children })?
        .map(|manifest| EncodedPage { manifest, payload: None }))
}
fn encode_manifest(scope: &ScopeKey, value: &ManifestPage) -> std::result::Result<Option<ManifestReply>, Error> {
    let Ok(bytes) = bounded_json(value, PAGE_BYTES as usize) else { return Ok(None) };
    let reply = ManifestReply {
        root: ManifestRoot(blob(scope, &bytes)),
        page_json: String::from_utf8(bytes).expect("JSON UTF-8"),
    };
    if wire_bytes(&reply)? > PAGE_BYTES as usize {
        return Ok(None);
    }
    manifest::validate(scope, reply.clone())?;
    Ok(Some(reply))
}
fn wire_bytes(reply: &ManifestReply) -> std::result::Result<usize, Error> {
    use crate::{
        cursor::{Holder, HoldingPeriod, Revision},
        identity::DeviceId,
        transfer::Message,
    };
    let holding = HoldingPeriod {
        holder: Holder::Device { device: DeviceId::from_bytes([0; 32]) },
        scope: reply.root.0.domain.clone(),
        acl_revision: Revision(u64::MAX),
        publication_epoch: Revision(u64::MAX),
    };
    // The underlying page is bounded before constructing its escaped envelope.
    Ok(serde_json::to_vec(&Message::Manifest { holding, reply: reply.clone() })
        .map_err(|_| Error::Invalid("manifest encoding failed".into()))?
        .len())
}
fn blob(scope: &ScopeKey, bytes: &[u8]) -> BlobId {
    BlobId { domain: scope.clone(), algorithm: DigestAlgorithm::Sha256, digest: Digest::of(bytes) }
}
fn unique(references: Vec<ContentRef>) -> std::result::Result<Vec<ContentRef>, Error> {
    let mut map = BTreeMap::new();
    for r in references {
        let key = (r.id().clone(), matches!(r, ContentRef::Manifest { .. }));
        if let Some(old) = map.insert(key, r.clone())
            && old != r
        {
            return Err(Error::Invalid("conflicting reference extents".into()));
        }
    }
    Ok(map.into_values().collect())
}
pub(super) fn corrupt(message: &str) -> Error {
    Error::Corrupt(message.into())
}
fn as_corrupt(error: Error) -> Error {
    match error {
        Error::Invalid(s) => Error::Corrupt(s),
        e => e,
    }
}
