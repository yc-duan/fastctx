//! Replay immutable local versions using durable submission receipts.

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
        || kept.text != source.text
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
    _all: &BTreeMap<String, Vec<(u64, PathBuf)>>,
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
    let destination = name.to_owned();
    if !accepted(link, transfer, owner, &destination, &first).await? {
        if link.conversation_history_stored(&transfer.account.account, owner, &destination, None).await?.is_some() {
            return Err("the hub already has an unrelated history with this name".into());
        }
        if known.len() >= super::limits::DOCUMENTS {
            return skip(
                node,
                records,
                transfer,
                name,
                format!("the history already has {} documents", super::limits::DOCUMENTS),
            );
        }
        transfer.names.insert(name.into(), destination.clone());
        state::keep(records, transfer).map_err(|error| error.to_string())?;
    }
    for (index, (version, path)) in versions.iter().enumerate() {
        let stored = state::stored(path, name, *version)?;
        if accepted(link, transfer, owner, &destination, &stored).await? {
            known.insert(destination.clone());
            continue;
        }
        let text = stored.text.clone();
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
