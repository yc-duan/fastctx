//! Typed public descriptors and bounded text transforms over validated spans.
use crate::conversation_ingest::{bodies::Bodies, parts, step_content::StepContent};
use fastctx_hosts::claude::ingest::document::{
    Span,
    read::{Reader, exact},
};
use fastctx_sync::{
    conversation::body_tree::{self, BodyExtent},
    store::Error,
};
use fastctx_types::sync::{
    PAGE_BYTES,
    conversation_timeline::*,
    identity::Digest,
    manifest::{BlobId, ContentRef, DigestAlgorithm, ManifestPage, ManifestReply, ManifestRoot},
    metadata::MetadataCodec,
    transport_codec::bounded_json,
};

pub async fn publish(io: &StepContent<'_>, fact: ItemFactV1) -> Result<ContentRef, Error> {
    let value = ItemFactBodyV1 { version: FACTS_VERSION, fact };
    value.validate_scope(&io.target().target().custody.object.scope).map_err(Error::Invalid)?;
    let payload = io.put_block(value.encode().map_err(Error::Invalid)?).await?;
    let page = ManifestPage::Metadata { codec: MetadataCodec::ItemFactsV1, payload, references: value.owned_content() };
    publish_page(io, page).await
}
/// The same public branch descriptor is used by display and typed part trees.
pub async fn part_branch(io: &StepContent<'_>, branch: &ItemDisplayPartsBranchV1) -> Result<ContentRef, Error> {
    let mut end = branch.start.0;
    if branch.version != DISPLAY_VERSION || branch.children.len() < 2 || branch.children.len() > FACT_PARTS {
        return Err(invalid("invalid facts part branch"));
    }
    let mut children = Vec::with_capacity(branch.children.len());
    for child in &branch.children {
        if child.start.0 != end
            || child.count.0 == 0
            || child.root.0.domain != io.target().target().custody.object.scope
        {
            return Err(invalid("facts part branch leaves a gap or crosses scope"));
        }
        end = end.checked_add(child.count.0).ok_or(Error::Busy)?;
        children.push(fastctx_types::sync::manifest::ManifestChild {
            root: child.root.clone(),
            first_key: format!("{:016x}", child.start.0),
            last_key: format!("{:016x}", end - 1),
            entries: child.count,
        });
    }
    if end.checked_sub(branch.start.0) != Some(branch.count.0) {
        return Err(invalid("facts branch count differs"));
    }
    publish_page(io, ManifestPage::Branch { children }).await
}
async fn publish_page(io: &StepContent<'_>, page: ManifestPage) -> Result<ContentRef, Error> {
    let bytes = bounded_json(&page, PAGE_BYTES as usize).map_err(invalid)?;
    let root = ManifestRoot(BlobId {
        domain: io.target().target().custody.object.scope.clone(),
        algorithm: DigestAlgorithm::Sha256,
        digest: Digest::of(&bytes),
    });
    io.put_manifest(ManifestReply { root: root.clone(), page_json: String::from_utf8(bytes).map_err(invalid)? })
        .await?;
    Ok(ContentRef::Manifest { root })
}
pub async fn load(io: &StepContent<'_>, reference: &ContentRef) -> Result<ItemFactBodyV1, Error> {
    let ContentRef::Manifest { root } = reference else { return Err(invalid("facts are not a metadata page")) };
    let page = io.public_manifest(root.clone()).await?;
    let ManifestPage::Metadata { codec: MetadataCodec::ItemFactsV1, payload, .. } =
        serde_json::from_str(&page.page_json).map_err(invalid)?
    else {
        return Err(invalid("facts descriptor has another codec"));
    };
    ItemFactBodyV1::decode(&io.public_block(payload).await?).map_err(Error::Invalid)
}
pub async fn text(io: &StepContent<'_>, reader: &Reader<'_>, span: Option<Span>) -> Result<ContentRef, Error> {
    match span {
        Some(span) => {
            parts::slice(io, &reader.document.bodies.strings, span).await.map(|body| body.content).map_err(super::error)
        }
        None => literal(io, "").await,
    }
}
pub async fn raw(io: &StepContent<'_>, reader: &Reader<'_>, span: Span) -> Result<ContentRef, Error> {
    parts::slice(io, &reader.document.bodies.raw, span).await.map(|body| body.content).map_err(super::error)
}
pub async fn literal(io: &StepContent<'_>, value: &str) -> Result<ContentRef, Error> {
    Ok(io.put_part(value.as_bytes().to_vec()).await?.into())
}
pub async fn optional_text(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    span: Option<Span>,
) -> Result<Option<ContentRef>, Error> {
    match span {
        Some(span) => text(io, reader, Some(span)).await.map(Some),
        None => Ok(None),
    }
}

pub struct TextWriter<'a, 'source> {
    io: &'a StepContent<'source>,
    before: Option<BodyExtent>,
    buffer: Vec<u8>,
}
impl<'a, 'source> TextWriter<'a, 'source> {
    pub fn new(io: &'a StepContent<'source>) -> Self {
        Self { io, before: None, buffer: Vec::with_capacity(64 * 1024) }
    }
    pub async fn push(&mut self, mut bytes: &[u8]) -> Result<(), Error> {
        while !bytes.is_empty() {
            let take = bytes.len().min(64 * 1024 - self.buffer.len());
            self.buffer.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.buffer.len() == 64 * 1024 {
                self.flush().await?;
            }
        }
        Ok(())
    }
    async fn flush(&mut self) -> Result<(), Error> {
        let block = self.io.put_part(std::mem::replace(&mut self.buffer, Vec::with_capacity(64 * 1024))).await?;
        self.before = Some(
            body_tree::append_block(&Bodies { content: self.io }, self.before.as_ref(), block)
                .await
                .map_err(super::tree_error)?,
        );
        Ok(())
    }
    pub async fn finish(mut self) -> Result<ContentRef, Error> {
        if !self.buffer.is_empty() || self.before.is_none() {
            self.flush().await?;
        }
        Ok(self.before.ok_or_else(|| invalid("text writer produced no extent"))?.content)
    }
    pub async fn span(&mut self, reader: &Reader<'_>, span: Span) -> Result<(), Error> {
        span.end().map_err(super::error)?;
        let mut at = 0;
        while at < span.bytes.0 {
            let bytes = exact(
                reader.input,
                &reader.document.bodies.strings,
                span.offset.0 + at,
                (span.bytes.0 - at).min(64 * 1024) as usize,
            )
            .await
            .map_err(super::error)?;
            self.push(&bytes).await?;
            at += bytes.len() as u64;
        }
        Ok(())
    }
    /// Mirrors str::lines, including CRLF and an unterminated final line, with
    /// only a byte of look-behind. UTF-8 sequences may cross source windows.
    pub async fn marked(&mut self, reader: &Reader<'_>, span: Option<Span>, mark: u8) -> Result<(), Error> {
        let Some(span) = span else { return Ok(()) };
        span.end().map_err(super::error)?;
        let mut at = 0;
        let mut active = false;
        let mut cr = false;
        while at < span.bytes.0 {
            let bytes = exact(
                reader.input,
                &reader.document.bodies.strings,
                span.offset.0 + at,
                (span.bytes.0 - at).min(64 * 1024) as usize,
            )
            .await
            .map_err(super::error)?;
            for &byte in &bytes {
                if byte == b'\n' {
                    if !active {
                        self.push(&[mark]).await?;
                    }
                    self.push(b"\n").await?;
                    active = false;
                    cr = false;
                } else {
                    if !active {
                        self.push(&[mark]).await?;
                        active = true;
                    }
                    if cr {
                        self.push(b"\r").await?;
                    }
                    cr = byte == b'\r';
                    if !cr {
                        self.push(&[byte]).await?;
                    }
                }
            }
            at += bytes.len() as u64;
        }
        if active {
            if cr {
                self.push(b"\r").await?;
            }
            self.push(b"\n").await?;
        }
        Ok(())
    }
}
fn invalid(error: impl std::fmt::Display) -> Error {
    Error::Invalid(error.to_string())
}
