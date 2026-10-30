//! Account records are independent of the directory. Each account incarnation
//! serializes its own operations; disk work never holds the registry lock.

pub(crate) mod history;
pub(crate) mod storage;

use std::sync::Arc;

use fastctx_net::authorization::Action;
use fastctx_net::sign::PublicKey;
use fastctx_net::wire::{Down, Told};
use fastctx_types::account_records::{
    Changed, MAX_ACCOUNT_BYTES, MAX_KIND_KEYS, Metadata, Record, Request, Response, words,
};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use serde_json::Value;

use crate::Hub;
pub(crate) use storage::Records;

pub(crate) fn authorized(
    hub: &Hub,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    opened: Timestamp,
) -> Result<(), String> {
    let store = hub.store();
    check(&store, from, key, account, opened)
}

pub(crate) fn check(
    store: &crate::Store,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    opened: Timestamp,
) -> Result<(), String> {
    let directory = &store.book.directory;
    directory.authorize(account, Action::ActFor(from)).map_err(|error| error.to_string())?;
    if directory.member(from).is_none_or(|member| member.key != *key) {
        return Err(words::DEVICE_REPLACED.into());
    }
    if directory.account(account).expect("authorized account").opened != opened {
        return Err(words::ACCOUNT_REPLACED.into());
    }
    Ok(())
}

/// Captures the account incarnation before dispatch, so an operation already
/// accepted cannot migrate to a replacement account while waiting for its turn.
pub(crate) fn begin(
    hub: &Hub,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    request: &Request,
) -> Result<Timestamp, String> {
    request.validate()?;
    let store = hub.store();
    let directory = &store.book.directory;
    directory.authorize(account, Action::ActFor(from)).map_err(|error| error.to_string())?;
    let opened = directory.account(account).expect("authorized account").opened;
    check(&store, from, key, account, opened)?;
    Ok(opened)
}

pub(crate) async fn ask(
    hub: &Arc<Hub>,
    from: MachineName,
    member_key: PublicKey,
    account: AccountName,
    opened: Timestamp,
    request: Request,
) -> Result<Told, String> {
    let slot = hub.account_records.account(&account, opened);
    let mut records = slot.records.clone().lock_owned().await;
    let hub = hub.clone();
    tokio::task::spawn_blocking(move || {
        authorized(&hub, &from, &member_key, &account, opened)?;
        for metadata in crate::skill_storage::recover(&slot, &mut records)? {
            tell(&hub, &account, opened, &metadata);
        }
        let response = match request {
            Request::List { kind } => Response::Listed {
                entries: records
                    .entries
                    .values()
                    .filter(|entry| entry.metadata.kind == kind && !entry.metadata.deleted)
                    .map(|entry| entry.metadata.clone())
                    .collect(),
            },
            Request::Read { kind, key } => {
                crate::record_subscriptions::follow(&hub, &from, &member_key, &account, &kind, &key)?;
                let record = if records.entries.contains_key(&(kind.clone(), key.clone())) {
                    let path = slot.path(&kind, &key);
                    Some(storage::read(&path)?)
                } else {
                    None
                };
                authorized(&hub, &from, &member_key, &account, opened)?;
                if kind == fastctx_types::skills::KIND
                    && !crate::record_subscriptions::folder_reader(&hub, &from, &member_key)
                    && let Some(record) = record.as_ref().filter(|record| !record.metadata.deleted)
                {
                    let skill: fastctx_types::skills::Skill =
                        serde_json::from_value(record.body.clone()).map_err(|error| error.to_string())?;
                    if fastctx_types::skill_files::requires_folder_editor(&skill)? {
                        return Err("update this client to read the complete skill folder".into());
                    }
                }
                if record.as_ref().is_some_and(|record| record.metadata.deleted) {
                    crate::record_subscriptions::deleted(&hub, &from, &account, &kind, &key);
                }
                Response::Read { record }
            }
            mutation => {
                let (kind, key, expected, mut body, deleted) = match mutation {
                    Request::Write { kind, key, expected, body } => (kind, key, expected, body, false),
                    Request::Delete { kind, key, expected } => (kind, key, expected, Value::Null, true),
                    _ => unreachable!("read operations handled above"),
                };
                let identity = (kind.clone(), key.clone());
                let before = records.entries.get(&identity);
                let version = before.map(|entry| entry.metadata.version);
                if expected != version {
                    return Ok(Told::AccountRecords {
                        response: Response::Conflict { current: before.map(|entry| entry.metadata.clone()) },
                    });
                }
                if kind == fastctx_types::skills::INDEX_KIND {
                    crate::skill_storage::check_index(&slot, &records, &key, &body, deleted)?;
                }
                if kind == fastctx_types::skills::KIND {
                    if deleted {
                        if let Some(before) = before.filter(|entry| !entry.metadata.deleted) {
                            let kept = storage::read(&slot.path(&kind, &key))?;
                            let skill: fastctx_types::skills::Skill =
                                serde_json::from_value(kept.body).map_err(|error| error.to_string())?;
                            if fastctx_types::skill_files::requires_folder_editor(&skill)? {
                                return Err(
                                    "use a folder-aware client to delete or rename a skill with auxiliary files".into(),
                                );
                            }
                            let _ = before;
                        }
                    } else {
                        if body.get("files").is_none_or(serde_json::Value::is_null) {
                            crate::skill_storage::legacy_edit(&slot, &records)?;
                        }
                        let incoming = serde_json::from_value(body).map_err(|error| error.to_string())?;
                        let skill = crate::skill_storage::normalize(&slot.path(&kind, &key), incoming)?;
                        if skill.name != key {
                            return Err("the skill name does not match its record key".into());
                        }
                        body = serde_json::to_value(&skill).map_err(|error| error.to_string())?;
                        crate::skill_storage::check_size(
                            &slot,
                            &records,
                            before,
                            &skill,
                            serde_json::to_vec(&body).map_err(|error| error.to_string())?.len(),
                        )?;
                    }
                }
                if !deleted
                    && kind != fastctx_types::skills::KIND
                    && before.is_none_or(|entry| entry.metadata.deleted)
                    && records
                        .entries
                        .values()
                        .filter(|entry| entry.metadata.kind == kind && !entry.metadata.deleted)
                        .count()
                        >= MAX_KIND_KEYS
                {
                    return Err(words::kind_limit());
                }
                let version = before.map_or(0, |entry| entry.metadata.version).checked_add(1).ok_or(words::REVISION)?;
                let record = Record {
                    metadata: Metadata { kind, key, version, by: from.clone(), at: Timestamp::now(), deleted },
                    body,
                };
                let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
                let size = bytes.len();
                let total = records.bytes - before.map_or(0, |entry| entry.size) + size;
                let ordinary = records
                    .entries
                    .values()
                    .filter(|entry| {
                        entry.metadata.kind != fastctx_types::skills::KIND
                            && entry.metadata.kind != fastctx_types::skills::INDEX_KIND
                    })
                    .map(|entry| entry.size)
                    .sum::<usize>();
                let specialized = record.metadata.kind == fastctx_types::skills::KIND
                    || record.metadata.kind == fastctx_types::skills::INDEX_KIND;
                let ordinary =
                    if specialized { ordinary } else { ordinary - before.map_or(0, |entry| entry.size) + size };
                if specialized {
                    let limits = crate::skill_storage::limits(&slot)?;
                    let held = records
                        .entries
                        .values()
                        .filter(|entry| {
                            entry.metadata.kind == fastctx_types::skills::KIND
                                || entry.metadata.kind == fastctx_types::skills::INDEX_KIND
                        })
                        .map(|entry| entry.size)
                        .sum::<usize>();
                    let next = held - before.map_or(0, |entry| entry.size) + size;
                    if next > held && next as u64 > limits.account_bytes {
                        return Err(format!(
                            "the account's skills would use {next} bytes; this hub's resource limit is {}",
                            limits.account_bytes
                        ));
                    }
                }
                if ordinary > MAX_ACCOUNT_BYTES {
                    return Err(words::account_limit(total));
                }
                let path = slot.path(&record.metadata.kind, &record.metadata.key);
                let (publishing, author, owner) = (hub.clone(), from.clone(), account.clone());
                let archive = record.metadata.kind == fastctx_types::plan::KIND;
                let historical = archive.then(|| slot.history(&record.metadata.kind, &record.metadata.key));
                let result = (move || {
                    fastctx_platform::ensure_private_dir(path.parent().expect("record folder"))
                        .map_err(|error| fastctx_platform::PublishFailure { error, published: false })?;
                    if let Some(history) = &historical {
                        history::preserve(&path, history)
                            .map_err(|error| fastctx_platform::PublishFailure { error, published: false })?;
                    }
                    let published = fastctx_platform::publish_checked(
                        &path,
                        &bytes,
                        fastctx_platform::Permissions::Private,
                        true,
                        || {
                            let store = publishing.store();
                            check(&store, &author, &member_key, &owner, opened).map_err(std::io::Error::other)?;
                            Ok(store)
                        },
                    );
                    if deleted
                        && (published.is_ok() || published.as_ref().is_err_and(|error| error.published))
                        && let Some(history) = historical
                    {
                        history::delete(&history)
                            .map_err(|error| fastctx_platform::PublishFailure { error, published: true })?;
                    }
                    published
                })();
                let failure = match result {
                    Ok(()) => None,
                    Err(failure) if failure.published => Some(failure.error.to_string()),
                    Err(failure) => return Err(words::store(failure.error)),
                };
                records.bytes = total;
                records.entries.insert(identity, storage::Entry { metadata: record.metadata.clone(), size });
                slot.count.store(records.entries.len(), std::sync::atomic::Ordering::Relaxed);
                slot.bytes.store(total, std::sync::atomic::Ordering::Relaxed);
                // Keep the account gate until hints are queued, preserving revision order.
                tell(&hub, &account, opened, &record.metadata);
                if let Some(why) = failure {
                    return Err(words::durability(version, why));
                }
                if !record.metadata.deleted
                    && record.metadata.kind == fastctx_types::sentinel_settings::RECORD_KIND
                    && record.metadata.key == fastctx_types::sentinel_settings::RECORD_KEY
                {
                    crate::sentinel_home::chosen(&hub, &from, &account, &record.body);
                }
                Response::Written { metadata: record.metadata }
            }
        };
        Ok(Told::AccountRecords { response })
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) fn tell(hub: &Hub, account: &AccountName, opened: Timestamp, metadata: &Metadata) {
    let readers: Vec<_> = hub
        .sessions()
        .iter()
        .filter(|(_, session)| {
            session.account_records && session.record_keys.as_ref().is_none_or(|keys| keys.wants(account, metadata))
        })
        .map(|(name, session)| (name.clone(), session.key, session.out.clone()))
        .collect();
    let store = hub.store();
    for (reader, key, out) in readers {
        if check(&store, &reader, &key, account, opened).is_ok() {
            let changed = Changed { account: account.clone(), metadata: metadata.clone() };
            let _ = out.send(crate::Out::Frame(Down::AccountRecordChanged { changed }));
        }
    }
}
