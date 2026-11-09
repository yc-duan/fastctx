//! Bounded semantic preparation over content already verified by the root reader.
//! No filesystem, block-store or writer calls occur in these pure functions.
use super::{Details, Document, Target, TextContent};
use crate::{Error, json};
use fastctx_sync::{
    catalog::ObjectHead,
    cursor::{ByteCount, VersionRef},
    identity::{Digest, ObjectKey},
    manifest::ContentRef,
    store::mutation::TextEdit,
};
use fastctx_types::{
    history, plan,
    write::{literal_edit, text},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validation {
    Stored,
    NewWrite,
}
/// Captured from the authenticated selected head before reading any referenced
/// content. The writer must recheck it before publishing the prepared result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fence {
    pub object: ObjectKey,
    pub revision: VersionRef,
    pub selected: VersionRef,
    pub description_root: ContentRef,
}
impl Fence {
    pub fn matches(&self, head: &ObjectHead) -> bool {
        self.object == head.key
            && self.revision == head.revision
            && self.selected == head.current_version
            && head.content.as_ref() == Some(&self.description_root)
            && head.lifecycle != fastctx_sync::catalog::Lifecycle::Deleted
    }
}
/// A borrowed, verified text view. Root closure verification precedes construction;
/// passing a matching root name alone is not proof of manifest membership.
pub struct Prepared<'a> {
    document: &'a Document,
    bytes: &'a [u8],
    fence: &'a Fence,
}
impl<'a> Prepared<'a> {
    pub fn document(&self) -> &'a Document {
        self.document
    }
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
    pub fn text(&self) -> &'a str {
        std::str::from_utf8(self.bytes).expect("prepared UTF-8")
    }
    pub fn fence(&self) -> &'a Fence {
        self.fence
    }
}
pub fn prepare<'a>(
    document: &'a Document,
    fence: &'a Fence,
    verified_root: &ContentRef,
    bytes: &'a [u8],
    max_bytes: usize,
    validation: Validation,
) -> Result<Prepared<'a>, Error> {
    document.validate()?;
    if bytes.len() > max_bytes {
        return Err(Error::Busy);
    }
    if verified_root != &document.text.root
        || fence.object.scope != document.target.object().scope
        || fence.description_root.id().domain != fence.object.scope
        || bytes.len() as u64 != document.text.bytes.0
        || Digest::of(bytes) != document.text.digest
        || bytes.starts_with(b"\xef\xbb\xbf") != document.text.utf8_bom
    {
        return Err(json::invalid("prepared text differs from its root, description or object fence"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| json::invalid("plans and history use UTF-8"))?;
    validate_text(document, text, validation)?;
    Ok(Prepared { document, bytes, fence })
}
pub fn validate_text(document: &Document, value: &str, validation: Validation) -> Result<(), Error> {
    match &document.details {
        Details::Plan { .. } => {
            if validation == Validation::NewWrite {
                plan::validate(value).map_err(Error::Invalid)?;
            }
        }
        Details::History { name, description, .. } => {
            let parsed = match validation {
                Validation::Stored => history::markdown::parse(name, value),
                Validation::NewWrite => history::markdown::parse_written(name, value),
            }
            .map_err(Error::Invalid)?;
            if &parsed.description != description {
                return Err(json::invalid("history description differs from its text"));
            }
        }
    }
    Ok(())
}
/// New text is validated against its own parsed description, without changing
/// header quotation, line endings or any other original bytes.
pub fn with_text(before: &Document, bytes: Vec<u8>, max_bytes: usize) -> Result<Candidate, Error> {
    create(before.target.clone(), before.details.clone(), bytes, max_bytes)
}
pub fn create(target: Target, mut details: Details, bytes: Vec<u8>, max_bytes: usize) -> Result<Candidate, Error> {
    if bytes.len() > max_bytes {
        return Err(Error::Busy);
    }
    let value = std::str::from_utf8(&bytes).map_err(|_| json::invalid("plans and history use UTF-8"))?;
    match &mut details {
        Details::Plan { .. } => {
            if !matches!(target, Target::Conversation { .. }) {
                return Err(json::invalid("a plan belongs to a conversation"));
            }
            plan::validate(value).map_err(Error::Invalid)?;
        }
        Details::History { name, description, .. } => {
            *description = history::markdown::parse_written(name, value).map_err(Error::Invalid)?.description
        }
    }
    Ok(Candidate { target, details, bytes })
}
/// Bytes must be put and durably pinned before attach. The returned description
/// contains only the resulting reference and digest, never an inline text copy.
pub struct Candidate {
    target: Target,
    details: Details,
    bytes: Vec<u8>,
}
pub enum Attribution {
    Plan(plan::Author),
    History(history::Author),
}
impl Candidate {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
    /// Supplied by the authenticated editor/legacy adapter, not the Markdown.
    /// The mutation authorizer checks this attribution against the real cause.
    pub fn attribute(&mut self, attribution: Attribution, submission: Option<String>) -> Result<(), Error> {
        match (&mut self.details, attribution) {
            (Details::Plan { author, submission: id }, Attribution::Plan(value)) => {
                *author = value;
                *id = submission;
            }
            (Details::History { author, submission: id, .. }, Attribution::History(value)) => {
                *author = value;
                *id = submission;
            }
            _ => return Err(json::invalid("attribution belongs to a different document kind")),
        }
        Ok(())
    }
    pub fn attach(self, root: ContentRef) -> Result<Document, Error> {
        let description = Document {
            target: self.target,
            details: self.details,
            text: TextContent {
                root,
                bytes: ByteCount(self.bytes.len() as u64),
                digest: Digest::of(&self.bytes),
                utf8_bom: self.bytes.starts_with(b"\xef\xbb\xbf"),
            },
        };
        description.validate()?;
        Ok(description)
    }
}
/// Whole-file writes preserve the existing BOM and dominant ending, as the file
/// tool does. This does not interpret an explicit non-UTF-8 encoding request.
pub fn write(before: &Prepared<'_>, content: &str, max_bytes: usize) -> Result<Candidate, Error> {
    if before.bytes.len() > max_bytes {
        return Err(Error::Busy);
    }
    let (bom, original) = split_bom(before.text());
    if content.len() > max_bytes {
        return Err(Error::Busy);
    }
    let ending = text::dominant_ending(original);
    let joined = content.matches("\r\n").count();
    let normalized = content.len().checked_sub(joined).ok_or(Error::Busy)?;
    let extra = if ending == "\r\n" { content.bytes().filter(|byte| *byte == b'\n').count() } else { 0 };
    if normalized.checked_add(extra).ok_or(Error::Busy)? > max_bytes.saturating_sub(bom.len()) {
        return Err(Error::Busy);
    }
    let value = text::newlines(content, ending);
    if value.len() > max_bytes.saturating_sub(bom.len()) {
        return Err(Error::Busy);
    }
    let mut bytes = Vec::with_capacity(bom.len() + value.len());
    bytes.extend_from_slice(bom.as_bytes());
    bytes.extend_from_slice(value.as_bytes());
    with_text(before.document, bytes, max_bytes)
}
/// All matching, ambiguity, overlap and unchanged-replacement decisions belong
/// to the shared literal editor. Every intermediate text observes the budget.
pub fn edit(before: &Prepared<'_>, edits: &[TextEdit], max_bytes: usize) -> Result<Candidate, Error> {
    if before.bytes.len() > max_bytes {
        return Err(Error::Busy);
    }
    let (bom, original) = split_bom(before.text());
    let mut value = original.to_owned();
    for edit in edits {
        value =
            literal_edit::apply(&value, &edit.search, &edit.replacement, edit.all, max_bytes.saturating_sub(bom.len()))
                .map_err(|error| match error {
                    literal_edit::Error::Match(reason) => Error::Invalid(reason),
                    literal_edit::Error::BudgetExceeded | literal_edit::Error::LengthOverflow => Error::Busy,
                })?;
    }
    let mut bytes = Vec::with_capacity(bom.len() + value.len());
    bytes.extend_from_slice(bom.as_bytes());
    bytes.extend_from_slice(value.as_bytes());
    with_text(before.document, bytes, max_bytes)
}
fn split_bom(text: &str) -> (&str, &str) {
    text.strip_prefix('\u{feff}').map_or(("", text), |rest| ("\u{feff}", rest))
}
