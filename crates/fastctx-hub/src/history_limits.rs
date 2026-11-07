//! New documents consume a scope's allowance; revisions and retired documents keep their place.

use fastctx_types::history::{MAX_DOCUMENTS, RECORD_KIND};

fn full() -> String {
    format!("this history has reached its limit of {MAX_DOCUMENTS} documents")
}

pub(crate) fn conversation(records: &crate::account_records::storage::Account, key: &str) -> Result<(), String> {
    if records.entries.get(&(RECORD_KIND.into(), key.into())).is_some_and(|entry| !entry.metadata.deleted) {
        return Ok(());
    }
    let (resident, name) = key.split_once('/').ok_or("a history record must name its conversation and document")?;
    fastctx_types::history::markdown::check_name(name)?;
    // Bound new allocations per history (2026-11-07), without scanning other conversations.
    if records.history(resident).filter(|entry| !entry.metadata.deleted).take(MAX_DOCUMENTS).count() >= MAX_DOCUMENTS {
        return Err(full());
    }
    Ok(())
}

pub(crate) fn project(index: &fastctx_net::topic_history::Index, name: &str) -> Result<(), String> {
    if index.latest(name).is_none() && index.document_count() >= MAX_DOCUMENTS {
        return Err(full());
    }
    Ok(())
}
