//! Loads one consistent composer authority, including forward-only legacy inputs.

use super::*;

pub(super) fn read(connection: &Connection, owner: &str, legacy_file: &std::path::Path) -> StoredResult<Stored> {
    let transaction = connection.unchecked_transaction()?;
    let row = transaction
        .query_row("SELECT revision,content FROM composers WHERE owner=?1", [owner], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .optional()?;
    let mut state: Stored = if let Some((revision, bytes)) = row {
        let mut state: Stored = serde_json::from_slice(&bytes).map_err(error)?;
        state.metadata_bytes = bytes.len();
        let revision = u64::try_from(revision).map_err(error)?;
        if state.revision > revision {
            return Err(error("composer metadata is newer than its durable revision"));
        }
        state.revision = revision;
        for document in state.documents_mut() {
            document.aliases_changed = false;
        }
        state.persisted = true;
        state
    } else {
        // Only the old node wrote this file; the conversation's cell
        // serializes its migration with edits and deletion now.
        match fastctx_platform::disk::read(legacy_file) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(error)?,
            Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => Stored::default(),
            Err(reason) => return Err(error(reason)),
        }
    };
    for document in state.documents_mut() {
        let key = key(owner, &document.id);
        let (loaded, external) = journal::load(&transaction, &key)?;
        document.load_aliases(journal::record(&transaction, owner, journal::ALIASES, &document.id)?).map_err(error)?;
        if !external && !loaded.checkpoint.is_empty() {
            document.checkpoint_migration = Some(loaded.checkpoint.clone());
        }
        document.hydrate(loaded).map_err(error)?;
    }
    state.document_names = state.documents().map(|document| document.id.clone()).collect();
    state.reindex();
    state.observed_documents = state.visible_document_ids();
    transaction.commit()?;
    Ok(state)
}
