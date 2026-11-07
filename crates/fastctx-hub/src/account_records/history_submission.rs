//! History retries return the accepted revision while the account gate is held.

use super::{history, storage};
use fastctx_types::{account_records::Metadata, history::stored::Stored};

pub(super) fn accepted(
    slot: &storage::Slot,
    records: &storage::Account,
    key: &str,
    body: &serde_json::Value,
) -> Result<Option<Metadata>, String> {
    let mut incoming: Stored = serde_json::from_value(body.clone()).map_err(|error| error.to_string())?;
    incoming.validate()?;
    if key.rsplit('/').next() != Some(incoming.revision.name.as_str()) {
        return Err("the history revision does not match its record key".into());
    }
    let Some(id) = incoming.submission.as_deref() else { return Ok(None) };
    let kind = fastctx_types::history::RECORD_KIND;
    let current = if records.entries.contains_key(&(kind.into(), key.into())) {
        Some(storage::read(&slot.path(kind, key))?)
    } else {
        return Ok(None);
    };
    let Some(record) = history::submitted(current.as_ref(), &slot.history(kind, key), id)? else {
        return Ok(None);
    };
    let kept: Stored = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
    incoming.revision.version = kept.revision.version;
    if incoming != kept {
        return Err("a history submission identity was reused for different content".into());
    }
    Ok(Some(record.metadata))
}
