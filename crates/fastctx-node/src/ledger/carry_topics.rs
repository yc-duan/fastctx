//! Classify whole history topics before copying any of their ledger entries.

use super::*;
use std::collections::BTreeSet;

pub(super) async fn skipped(
    link: &Link,
    account: &AccountName,
    town: &TownId,
    entries: &[(u64, Entry, Signed)],
) -> Result<BTreeMap<String, String>, String> {
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    let mut skipped = BTreeMap::new();
    for (_, entry, _) in entries {
        if let Some(stored) = fastctx_net::topic_history::decode(&entry.files)? {
            let name = stored.revision.name.clone();
            if let Some(reason) = crate::history::import::limits::description(&stored.revision) {
                skipped.entry(name.clone()).or_insert(reason);
            }
            if seen.insert(name.clone()) {
                names.push(name);
            }
        }
    }
    if names.is_empty() {
        return Ok(skipped);
    }
    let has_ledger = link.ledger_catalog().await?.iter().any(|(id, latest)| id == town && *latest > 0);
    let existing: BTreeSet<_> = if has_ledger {
        link.history_catalog(account, town).await?.documents.into_iter().map(|revision| revision.name).collect()
    } else {
        BTreeSet::new()
    };
    let mut count = existing.len();
    for name in names {
        if skipped.contains_key(&name) || existing.contains(&name) {
            continue;
        }
        let limit = crate::history::import::limits::DOCUMENTS;
        if count >= limit {
            skipped.insert(name, format!("the history would exceed {limit} documents"));
        } else {
            count += 1;
        }
    }
    Ok(skipped)
}

pub(super) fn omitted(entry: &Entry, skipped: &BTreeMap<String, String>) -> Result<bool, String> {
    Ok(fastctx_net::topic_history::decode(&entry.files)?
        .is_some_and(|stored| skipped.contains_key(&stored.revision.name)))
}
