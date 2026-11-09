//! Text document handlers share the file tool's pure literal-edit implementation.
use crate::store::{Error, mutation::TextEdit};

/// Applies sequential edits to a schema whose canonical content is UTF-8.
/// Other encodings must be decoded by their own handler before matching.
pub fn apply_edits(current: &[u8], edits: &[TextEdit], max_bytes: usize) -> Result<Vec<u8>, Error> {
    let mut text = std::str::from_utf8(current).map_err(|e| Error::Invalid(e.to_string()))?.to_owned();
    if text.len() > max_bytes {
        return Err(Error::Busy);
    }
    for edit in edits {
        text = fastctx_types::write::literal_edit::apply(&text, &edit.search, &edit.replacement, edit.all, max_bytes)
            .map_err(Error::Invalid)?;
    }
    Ok(text.into_bytes())
}
