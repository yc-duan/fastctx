//! Folder-aware skill operations. A read returns the version the editor must
//! carry back; the folder, an optional rename, and its index commit together.

use crate::{
    Hub,
    account_records::{
        self,
        storage::{self, Account, Slot},
    },
};
use fastctx_net::{sign::PublicKey, wire::Told};
use fastctx_types::{
    AccountName, MachineName,
    account_records::{Metadata, Record},
    skill_files::{Reply, Request, Snapshot},
    skills::{self, Index, Listed, Skill},
};
use jiff::Timestamp;
use std::sync::Arc;

fn read(slot: &Slot, account: &Account, kind: &str, key: &str) -> Result<Option<Record>, String> {
    if account.entries.contains_key(&(kind.into(), key.into())) {
        storage::read(&slot.path(kind, key)).map(Some)
    } else {
        Ok(None)
    }
}
fn index(slot: &Slot, account: &Account) -> Result<(Index, Option<Metadata>), String> {
    let record = read(slot, account, skills::INDEX_KIND, skills::INDEX_KEY)?;
    match record {
        Some(record) if !record.metadata.deleted => {
            Ok((serde_json::from_value(record.body).map_err(|error| error.to_string())?, Some(record.metadata)))
        }
        Some(record) => Ok((Index::default(), Some(record.metadata))),
        None => Ok((Index::default(), None)),
    }
}
fn skill(record: &Option<Record>) -> Result<Option<Skill>, String> {
    record
        .as_ref()
        .filter(|record| !record.metadata.deleted)
        .map(|record| serde_json::from_value(record.body.clone()).map_err(|error| error.to_string()))
        .transpose()
}
fn metadata(
    from: &MachineName,
    kind: &str,
    key: &str,
    previous: Option<&Metadata>,
    deleted: bool,
) -> Result<Metadata, String> {
    Ok(Metadata {
        kind: kind.into(),
        key: key.into(),
        version: previous.map_or(0, |metadata| metadata.version).checked_add(1).ok_or("skill revision exhausted")?,
        by: from.clone(),
        at: Timestamp::now(),
        deleted,
    })
}

pub(crate) async fn ask(
    hub: &Arc<Hub>,
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    opened: Timestamp,
    request: Request,
) -> Result<Told, String> {
    let slot = hub.account_records.account(&account, opened);
    let mut records = slot.records.clone().lock_owned().await;
    let hub = hub.clone();
    tokio::task::spawn_blocking(move || {
        request.validate()?;
        account_records::authorized(&hub, &from, &key, &account, opened)?;
        for metadata in crate::skill_storage::recover(&slot, &mut records)? {
            account_records::tell(&hub, &account, opened, &metadata);
        }
        if let Some(name) = request.name() {
            crate::record_subscriptions::follow(&hub, &from, &key, &account, skills::KIND, name)?;
        }
        let limits = crate::skill_storage::limits(&slot)?;
        let result = match request {
            Request::Limits => Reply::Limits { limits },
            Request::List => Reply::Listed { skills: index(&slot, &records)?.0.skills },
            Request::Read { name } => {
                let record = read(&slot, &records, skills::KIND, &name)?;
                if record.as_ref().is_some_and(|record| record.metadata.deleted) {
                    crate::record_subscriptions::deleted(&hub, &from, &account, skills::KIND, &name);
                }
                let mut body = skill(&record)?;
                if let Some(skill) = &mut body {
                    skill.files = Some(skill.folder()?);
                }
                Reply::Read {
                    snapshot: Box::new(Snapshot { metadata: record.map(|record| record.metadata), skill: body }),
                }
            }
            Request::ReadFile { name, path } => {
                let record = read(&slot, &records, skills::KIND, &name)?;
                let body = skill(&record)?.ok_or_else(|| skills::words::missing(&name))?;
                Reply::File {
                    version: record.expect("a skill has a record").metadata.version,
                    file: body.folder()?.into_iter().find(|file| file.path == path),
                }
            }
            mutation => {
                let name = mutation.name().expect("mutation has a name").to_string();
                let current = read(&slot, &records, skills::KIND, &name)?;
                let previous = current.as_ref().map(|record| &record.metadata);
                let expected = match &mutation {
                    Request::Write { expected, .. } => *expected,
                    Request::Edit { expected, .. } | Request::Delete { expected, .. } => Some(*expected),
                    _ => unreachable!(),
                };
                if expected != previous.map(|metadata| metadata.version) {
                    return Ok(Told::SkillFiles { reply: Reply::Conflict { current: previous.cloned() } });
                }
                let replacement = match mutation {
                    Request::Write { files, .. } => Some(fastctx_types::skill_files::from_files(files)?),
                    Request::Edit { changes, .. } => {
                        let old = skill(&current)?.ok_or_else(|| skills::words::missing(&name))?;
                        Some(fastctx_types::skill_files::from_files(fastctx_types::skill_files::edit(
                            old.folder()?,
                            changes,
                        )?)?)
                    }
                    Request::Delete { .. } => None,
                    _ => unreachable!(),
                };
                let renamed = replacement.as_ref().is_some_and(|skill| skill.name != name);
                if renamed && current.as_ref().is_none_or(|record| record.metadata.deleted) {
                    return Err("the imported skill's name does not match its requested name".into());
                }
                let target = replacement.as_ref().map_or(name.as_str(), |skill| skill.name.as_str());
                let destination = if renamed { read(&slot, &records, skills::KIND, target)? } else { current.clone() };
                if renamed && destination.as_ref().is_some_and(|record| !record.metadata.deleted) {
                    return Err(format!("a skill named {target} already exists"));
                }
                let saved = metadata(
                    &from,
                    skills::KIND,
                    target,
                    destination.as_ref().map(|record| &record.metadata),
                    replacement.is_none(),
                )?;
                let mut changes = vec![Record {
                    metadata: saved.clone(),
                    body: serde_json::to_value(&replacement).map_err(|error| error.to_string())?,
                }];
                if renamed {
                    changes.push(Record {
                        metadata: metadata(&from, skills::KIND, &name, previous, true)?,
                        body: serde_json::Value::Null,
                    });
                }
                let (mut catalog, catalog_before) = index(&slot, &records)?;
                catalog.skills.retain(|skill| skill.name != name && skill.name != target);
                if let Some(skill) = &replacement {
                    catalog.skills.push(Listed { name: skill.name.clone(), description: skill.description.clone() });
                }
                catalog.skills.sort_by(|a, b| a.name.cmp(&b.name));
                changes.push(Record {
                    metadata: metadata(&from, skills::INDEX_KIND, skills::INDEX_KEY, catalog_before.as_ref(), false)?,
                    body: serde_json::to_value(catalog).map_err(|error| error.to_string())?,
                });
                let mut total = records
                    .entries
                    .values()
                    .filter(|entry| entry.metadata.kind == skills::KIND || entry.metadata.kind == skills::INDEX_KIND)
                    .map(|entry| entry.size)
                    .sum::<usize>();
                let held = total;
                for change in &changes {
                    let bytes = serde_json::to_vec(change).map_err(|error| error.to_string())?.len();
                    if bytes > fastctx_types::skill_files::ENCODED_LIMIT {
                        return Err("the encoded skill folder or index exceeds 8 MiB".into());
                    }
                    total = total
                        - records
                            .entries
                            .get(&(change.metadata.kind.clone(), change.metadata.key.clone()))
                            .map_or(0, |entry| entry.size)
                        + bytes;
                }
                if total > held && total as u64 > limits.account_bytes {
                    return Err(format!(
                        "the account's skills would use {total} bytes; this hub's resource limit is {}",
                        limits.account_bytes
                    ));
                }
                if let Some(skill) = &replacement {
                    let files = skill.folder()?;
                    if fastctx_types::skill_files::validate_files(&files)? > limits.skill_bytes
                        || files.len() as u64 > limits.files
                    {
                        return Err(format!(
                            "the skill exceeds this hub's resource limits ({} bytes, {} files)",
                            limits.skill_bytes, limits.files
                        ));
                    }
                }
                fastctx_platform::ensure_private_dir(slot.root()).map_err(|error| error.to_string())?;
                let path = crate::skill_storage::intent(&slot);
                let bytes = serde_json::to_vec(&changes).map_err(|error| error.to_string())?;
                let (publishing, writer, who) = (hub.clone(), from.clone(), account.clone());
                fastctx_platform::publish_checked(&path, &bytes, fastctx_platform::Permissions::Private, true, || {
                    let store = publishing.store();
                    account_records::check(&store, &writer, &key, &who, opened).map_err(std::io::Error::other)?;
                    Ok(store)
                })
                .map_err(|error| format!("committing the skill folder: {}", error.error))?;
                for metadata in crate::skill_storage::recover(&slot, &mut records)? {
                    account_records::tell(&hub, &account, opened, &metadata);
                }
                Reply::Written { metadata: saved }
            }
        };
        account_records::authorized(&hub, &from, &key, &account, opened)?;
        Ok(Told::SkillFiles { reply: result })
    })
    .await
    .map_err(|error| error.to_string())?
}
