//! Replay immutable local versions, reserving a different name when the hub already has one.

use super::state::{self, Transfer};
use crate::Node;
use fastctx_net::link::Link;
use fastctx_types::{
    Address,
    history::stored::Stored,
    owned_records::{Request, Response},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

fn identity(owner: &Address, source: &Stored) -> Result<String, String> {
    let bytes = serde_json::to_vec(&("standalone-history-v1", &owner.resident, &source.revision, &source.text))
        .map_err(|error| error.to_string())?;
    Ok(fastctx_net::sign::sha256(&bytes).iter().map(|byte| format!("{byte:02x}")).collect())
}

fn renamed(source: &Stored, name: &str) -> String {
    if source.revision.name == name {
        return source.text.clone();
    }
    let mut result = String::with_capacity(source.text.len() + name.len());
    let mut header = true;
    for (index, raw) in source.text.split_inclusive('\n').enumerate() {
        let line = raw.trim_end_matches(['\r', '\n']);
        if index > 0 && header && line == "---" {
            header = false;
        }
        if header && line.starts_with("name:") {
            result.push_str("name: ");
            result.push_str(name);
            result.push_str(&raw[line.len()..]);
        } else {
            result.push_str(raw);
        }
    }
    result
}

fn candidate(name: &str, number: u64) -> String {
    if number == 0 {
        return name.into();
    }
    let suffix = if number == 1 { "-imported".into() } else { format!("-imported-{number}") };
    format!("{}{}", &name[..name.len().min(240 - suffix.len())], suffix)
}

async fn accepted(
    link: &Link,
    transfer: &Transfer,
    owner: &Address,
    name: &str,
    source: &Stored,
) -> Result<bool, String> {
    let id = identity(owner, source)?;
    let response = link
        .owned_records(
            &transfer.account.account,
            Request::Submitted {
                conversation: owner.clone(),
                kind: fastctx_types::history::RECORD_KIND.into(),
                key: format!("{}/{name}", owner.resident),
                submission: id.clone(),
            },
        )
        .await?;
    let Response::Submitted { record } = response else {
        return Err("the hub did not return an import receipt".into());
    };
    let Some(record) = record else { return Ok(false) };
    let kept: Stored = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
    kept.validate()?;
    // The reader's rights may redact machine coordinates, but never these original fields.
    if kept.submission.as_deref() != Some(&id)
        || kept.text != renamed(source, name)
        || kept.revision.name != name
        || kept.revision.retired != source.revision.retired
        || kept.revision.description != source.revision.description
        || kept.revision.author.stamp.at != source.revision.author.stamp.at
        || kept.revision.author.host != source.revision.author.host
        || kept.revision.author.model != source.revision.author.model
    {
        return Err("the accepted history submission differs from its standalone source".into());
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn topic(
    node: &Node,
    link: &Link,
    owner: &Address,
    records: &Path,
    transfer: &mut Transfer,
    name: &str,
    versions: &[(u64, PathBuf)],
    all: &BTreeMap<String, Vec<(u64, PathBuf)>>,
    known: &mut BTreeSet<String>,
) -> Result<(), String> {
    if transfer.skipped.contains_key(name) {
        return Ok(());
    }
    for (version, path) in versions {
        let stored = state::stored(path, name, *version)?;
        if let Some(reason) = super::limits::description(&stored.revision) {
            return skip(node, records, transfer, name, reason);
        }
    }
    let first = state::stored(&versions[0].1, name, versions[0].0)?;
    let mut number = 0u64;
    let destination = loop {
        let destination = transfer.names.get(name).cloned().unwrap_or_else(|| candidate(name, number));
        if accepted(link, transfer, owner, &destination, &first).await? {
            known.insert(destination.clone());
            break destination;
        }
        let reserved = destination != name
            && (all.contains_key(&destination)
                || transfer.names.iter().any(|(source, target)| source != name && target == &destination));
        if !reserved
            && link.conversation_history_stored(&transfer.account.account, owner, &destination, None).await?.is_none()
        {
            if known.len() >= super::limits::DOCUMENTS {
                return skip(
                    node,
                    records,
                    transfer,
                    name,
                    format!("the history already has {} documents", super::limits::DOCUMENTS),
                );
            }
            // Check every renamed version before creating even the first one.
            for (version, path) in versions {
                let stored = state::stored(path, name, *version)?;
                if renamed(&stored, &destination).len() > fastctx_types::history::MAX_DOCUMENT_BYTES {
                    let reason = format!("renaming to {destination} exceeds the history size limit");
                    return skip(node, records, transfer, name, reason);
                }
            }
            transfer.names.insert(name.into(), destination.clone());
            state::keep(records, transfer).map_err(|error| error.to_string())?;
            if destination != name {
                node.log.line(format_args!(
                    "importing {} history {name} as {destination}: the hub already has that name",
                    owner.resident
                ));
            }
            break destination;
        }
        // Another importer may have claimed the name between the receipt and head reads.
        if accepted(link, transfer, owner, &destination, &first).await? {
            known.insert(destination.clone());
            break destination;
        }
        transfer.names.remove(name);
        number = number.checked_add(1).ok_or("history import names are exhausted")?;
    };
    for (index, (version, path)) in versions.iter().enumerate() {
        let stored = state::stored(path, name, *version)?;
        if accepted(link, transfer, owner, &destination, &stored).await? {
            known.insert(destination.clone());
            continue;
        }
        let text = renamed(&stored, &destination);
        let mut saved = false;
        for _ in 0..8 {
            let current =
                link.conversation_history_stored(&transfer.account.account, owner, &destination, None).await?;
            // A concurrent acceptance is now either in the receipt or ahead of this CAS base.
            if accepted(link, transfer, owner, &destination, &stored).await? {
                saved = true;
                break;
            }
            if index == 0 && current.is_some() {
                return Err("a history name was taken while its import was starting; retrying with another name".into());
            }
            if !super::super::switch::enabled(node, &transfer.account) {
                return Err(super::super::switch::COPIES_OFF.into());
            }
            let result = link
                .conversation_history_save(
                    &transfer.account.account,
                    owner,
                    &destination,
                    text.clone(),
                    current.map(|stored| stored.revision.version),
                    stored.revision.retired,
                    stored.revision.author.clone(),
                    Some(identity(owner, &stored)?),
                )
                .await;
            if accepted(link, transfer, owner, &destination, &stored).await? {
                saved = true;
                break;
            }
            if let Err(error) = result
                && !error.starts_with("the document changed;")
            {
                if index == 0
                    && link.conversation_history_catalog(&transfer.account.account, owner).await?.documents.len()
                        >= super::limits::DOCUMENTS
                {
                    return skip(
                        node,
                        records,
                        transfer,
                        name,
                        format!("the history already has {} documents", super::limits::DOCUMENTS),
                    );
                }
                return Err(error);
            }
        }
        if !saved {
            return Err("history kept changing during its import; retrying later".into());
        }
        known.insert(destination.clone());
    }
    Ok(())
}

fn skip(node: &Node, records: &Path, transfer: &mut Transfer, name: &str, reason: String) -> Result<(), String> {
    transfer.skipped.insert(name.into(), reason.clone());
    state::keep(records, transfer).map_err(|error| error.to_string())?;
    node.log.line(format_args!(
        "history {name} was not imported: {reason}; originals remain at {}",
        records.join(name).display()
    ));
    Ok(())
}
