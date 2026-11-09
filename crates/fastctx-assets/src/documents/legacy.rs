//! Pure old-shape decoding and explicit version bindings. The migration owner
//! preserves original bytes and commits mappings before exposing projections.
use super::{Details, Document, Target, TextContent, text::Prepared};
use crate::{Error, json};
use fastctx_sync::{
    cursor::{ByteCount, VersionRef},
    identity::{Digest, ObjectKey},
    manifest::ContentRef,
};
use fastctx_types::{
    Address,
    account_records::{Metadata, Record, Request},
    history::{self, stored::Stored},
    plan,
};
use serde::{Deserialize, Serialize};

/// A decoded body is not proof of account ownership or a canonical identity.
/// The caller supplies the independently resolved target when attaching content.
pub struct Decoded<'a> {
    pub text: Option<String>,
    pub details: Option<Details>,
    pub metadata: Option<Metadata>,
    pub version: u64,
    pub original: &'a [u8],
}
impl Decoded<'_> {
    pub fn attach(&self, target: Target, root: ContentRef) -> Result<Document, Error> {
        let text = self.text.as_ref().ok_or_else(|| json::invalid("a tombstone has no text content"))?;
        let details = self.details.clone().ok_or_else(|| json::invalid("a tombstone has no document description"))?;
        let document = Document {
            target,
            details,
            text: TextContent {
                root,
                bytes: ByteCount(text.len() as u64),
                digest: Digest::of(text.as_bytes()),
                utf8_bom: text.starts_with('\u{feff}'),
            },
        };
        document.validate()?;
        Ok(document)
    }
}

pub fn plan_record<'a>(bytes: &'a [u8], owner: &Address, max_bytes: usize) -> Result<Decoded<'a>, Error> {
    if bytes.len() > max_bytes {
        return Err(Error::Busy);
    }
    let record: Record = serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
    if record.metadata.kind != plan::KIND
        || record.metadata.key != owner.resident.to_string()
        || record.metadata.version == 0
    {
        return Err(json::invalid("legacy plan key or revision differs from its resolved owner"));
    }
    if record.metadata.deleted {
        if !record.body.is_null() {
            return Err(json::invalid("a legacy tombstone must have a null body"));
        }
        return Ok(Decoded {
            text: None,
            details: None,
            version: record.metadata.version,
            metadata: Some(record.metadata),
            original: bytes,
        });
    }
    // Old generic account writes used this limit, which is not identical to the
    // newer typed plan writer's escaped-text limit. Import does not rewrite it.
    Request::Write {
        kind: plan::KIND.into(),
        key: record.metadata.key.clone(),
        expected: None,
        body: record.body.clone(),
    }
    .validate()
    .map_err(Error::Invalid)?;
    let body: plan::Body = serde_json::from_value(record.body).map_err(|e| Error::Invalid(e.to_string()))?;
    if body.of().is_some_and(|found| found != owner) {
        return Err(json::invalid("legacy plan belongs to another conversation"));
    }
    Ok(Decoded {
        text: Some(body.markdown),
        details: Some(Details::Plan { author: body.author, submission: body.submission.map(|id| *id) }),
        version: record.metadata.version,
        metadata: Some(record.metadata),
        original: bytes,
    })
}

pub fn history_record<'a>(
    bytes: &'a [u8],
    owner: &Address,
    name: &str,
    max_bytes: usize,
) -> Result<Decoded<'a>, Error> {
    if bytes.len() > max_bytes {
        return Err(Error::Busy);
    }
    let record: Record = serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
    if record.metadata.kind != history::RECORD_KIND
        || record.metadata.key != format!("{}/{name}", owner.resident)
        || record.metadata.version == 0
    {
        return Err(json::invalid("legacy history key or revision differs from its resolved owner"));
    }
    if record.metadata.deleted {
        if !record.body.is_null() {
            return Err(json::invalid("a legacy tombstone must have a null body"));
        }
        return Ok(Decoded {
            text: None,
            details: None,
            version: record.metadata.version,
            metadata: Some(record.metadata),
            original: bytes,
        });
    }
    let stored: Stored = serde_json::from_value(record.body).map_err(|e| Error::Invalid(e.to_string()))?;
    stored.validate().map_err(Error::Invalid)?;
    if stored.revision.version != record.metadata.version || stored.revision.name != name {
        return Err(json::invalid("legacy history body and record metadata disagree"));
    }
    let details = history_details(&stored);
    Ok(Decoded {
        text: Some(stored.text),
        details: Some(details),
        version: record.metadata.version,
        metadata: Some(record.metadata),
        original: bytes,
    })
}
fn history_details(stored: &Stored) -> Details {
    Details::History {
        name: stored.revision.name.clone(),
        description: stored.revision.description.clone(),
        retired: stored.revision.retired,
        author: stored.revision.author.clone(),
        submission: stored.submission.clone(),
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRevision {
    #[serde(flatten)]
    pub revision: history::HistoryRevision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission: Option<String>,
}
/// The adapter verifies the signed ledger, exact two paths and project identity;
/// this function checks that their decoded name/version/text agree as before.
pub fn project_history<'a>(
    metadata: &'a [u8],
    body: &[u8],
    name: &str,
    version: u64,
    max_bytes: usize,
) -> Result<Decoded<'a>, Error> {
    if metadata.len().checked_add(body.len()).ok_or(Error::Busy)? > max_bytes {
        return Err(Error::Busy);
    }
    let revision: ProjectRevision = serde_json::from_slice(metadata).map_err(|e| Error::Invalid(e.to_string()))?;
    let text = std::str::from_utf8(body).map_err(|_| json::invalid("history text is not UTF-8"))?;
    let stored = Stored { revision: revision.revision, submission: revision.submission, text: text.into() };
    stored.validate().map_err(Error::Invalid)?;
    if stored.revision.name != name || stored.revision.version != version {
        return Err(json::invalid("project history paths and revision differ"));
    }
    let details = history_details(&stored);
    Ok(Decoded { text: Some(stored.text), details: Some(details), metadata: None, version, original: metadata })
}

/// One persisted mapping supplied by the migration/compatibility service. Old
/// numbers are never produced by truncating or sorting a VersionRef revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionBinding {
    pub object: ObjectKey,
    pub accepted: VersionRef,
    pub description_root: ContentRef,
    pub legacy_version: u64,
}
impl VersionBinding {
    pub fn check(&self, prepared: &Prepared<'_>) -> Result<(), Error> {
        if self.legacy_version == 0
            || self.object != prepared.fence().object
            || self.accepted != prepared.fence().selected
            || self.description_root != prepared.fence().description_root
        {
            return Err(Error::Unavailable("no exact legacy mapping for this selected version".into()));
        }
        Ok(())
    }
}
pub fn plan_body(
    prepared: &Prepared<'_>,
    mapping: &VersionBinding,
    owner: Option<Address>,
) -> Result<plan::Body, Error> {
    mapping.check(prepared)?;
    let Details::Plan { author, submission } = &prepared.document().details else {
        return Err(json::invalid("not a plan"));
    };
    Ok(plan::Body {
        markdown: prepared.text().into(),
        author: author.clone(),
        owner: owner.map(Box::new),
        submission: submission.clone().map(Box::new),
    })
}
pub fn history_body(prepared: &Prepared<'_>, mapping: &VersionBinding) -> Result<Stored, Error> {
    mapping.check(prepared)?;
    let Details::History { name, description, retired, author, submission } = &prepared.document().details else {
        return Err(json::invalid("not history"));
    };
    let stored = Stored {
        revision: history::HistoryRevision {
            name: name.clone(),
            description: description.clone(),
            version: mapping.legacy_version,
            retired: *retired,
            author: author.clone(),
        },
        text: prepared.text().into(),
        submission: submission.clone(),
    };
    stored.validate().map_err(Error::Invalid)?;
    Ok(stored)
}
pub fn project_revision(
    prepared: &Prepared<'_>,
    mapping: &VersionBinding,
) -> Result<(ProjectRevision, Vec<u8>), Error> {
    let stored = history_body(prepared, mapping)?;
    Ok((ProjectRevision { revision: stored.revision, submission: stored.submission }, stored.text.into_bytes()))
}
