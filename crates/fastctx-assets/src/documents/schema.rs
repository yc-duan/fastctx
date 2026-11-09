//! New descriptions are bounded independently of their referenced text. Legacy
//! preservation uses a separate entry and never grants permission for a new write.
use super::{Details, Document};
use crate::{Error, json};
use fastctx_types::{history, plan};
use serde::Serialize;

pub const DESCRIPTION_BYTES: usize = 64 * 1024;
pub const AUTHOR_BYTES: usize = 16 * 1024;
pub const AUTHOR_FIELD_BYTES: usize = 4096;
/// Includes the old account-record envelope allowance and the new content key.
pub const LEGACY_DESCRIPTION_BYTES: usize = plan::MAX_KEPT_BYTES + 32 * 1024;

pub(super) fn validate(document: &Document, stored: bool) -> Result<(), Error> {
    encoded_bound(document, if stored { LEGACY_DESCRIPTION_BYTES } else { DESCRIPTION_BYTES })?;
    match &document.details {
        Details::Plan { author, submission } => {
            let limit = plan::MAX_KEPT_BYTES + if stored { 16 * 1024 } else { 0 };
            if document.text.bytes.0 > limit as u64 {
                return Err(json::invalid("plan text extent exceeds its schema bound"));
            }
            if !stored {
                attribution(author)?;
                submission_id(submission)?;
            }
        }
        Details::History { name, description, author, submission, .. } => {
            history::markdown::check_name(name).map_err(Error::Invalid)?;
            let limit = if stored { history::MAX_DOCUMENT_BYTES } else { history::MAX_DESCRIPTION_BYTES };
            if description.len() > limit || description.trim().is_empty() || description.chars().any(char::is_control) {
                return Err(json::invalid("history description must fit its bound and be a nonempty single line"));
            }
            if document.text.utf8_bom {
                return Err(json::invalid("a history document cannot have a UTF-8 BOM"));
            }
            if document.text.bytes.0 > history::MAX_DOCUMENT_BYTES as u64 {
                return Err(json::invalid("history text extent exceeds its schema bound"));
            }
            submission_id(submission)?;
            if !stored {
                attribution(author)?;
            }
        }
    }
    Ok(())
}
pub(super) fn submission_id(submission: &Option<String>) -> Result<(), Error> {
    if submission.as_ref().is_some_and(|id| id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit())) {
        Err(json::invalid("invalid document submission identity"))
    } else {
        Ok(())
    }
}
pub(super) fn attribution(author: &impl Serialize) -> Result<(), Error> {
    encoded_bound(author, AUTHOR_BYTES)?;
    let value = serde_json::to_value(author).map_err(|_| json::invalid("invalid document attribution"))?;
    fn fields(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::String(value) => value.len() <= AUTHOR_FIELD_BYTES,
            serde_json::Value::Array(values) => values.iter().all(fields),
            serde_json::Value::Object(values) => values.values().all(fields),
            _ => true,
        }
    }
    if !fields(&value) {
        return Err(json::invalid("document attribution field exceeds its schema bound"));
    }
    Ok(())
}
fn encoded_bound(value: &impl Serialize, max: usize) -> Result<(), Error> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.checked_sub(bytes.len()).ok_or_else(|| std::io::Error::other("description size bound"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(max), value)
        .map_err(|_| json::invalid("document description exceeds its schema bound"))
}
