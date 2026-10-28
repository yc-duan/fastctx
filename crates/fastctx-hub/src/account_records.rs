//! Account records are independent of the directory. Each account incarnation
//! serializes its own operations; disk work never holds the registry lock.

mod storage;

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

fn authorized(
    hub: &Hub,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    opened: Timestamp,
) -> Result<(), String> {
    let store = hub.store();
    check(&store, from, key, account, opened)
}

fn check(
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
    let mut records = slot.records.lock().await;
    authorized(hub, &from, &member_key, &account, opened)?;
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
            let record = if records.entries.contains_key(&(kind.clone(), key.clone())) {
                let path = slot.path(&kind, &key);
                Some(
                    tokio::task::spawn_blocking(move || storage::read(&path))
                        .await
                        .map_err(|error| error.to_string())??,
                )
            } else {
                None
            };
            authorized(hub, &from, &member_key, &account, opened)?;
            Response::Read { record }
        }
        mutation => {
            let (kind, key, expected, body, deleted) = match mutation {
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
            if !deleted
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
            if total > MAX_ACCOUNT_BYTES {
                return Err(words::account_limit(total));
            }
            let path = slot.path(&record.metadata.kind, &record.metadata.key);
            let (publishing, author, owner) = (hub.clone(), from.clone(), account.clone());
            let result = tokio::task::spawn_blocking(move || {
                fastctx_platform::ensure_private_dir(path.parent().expect("record folder"))
                    .map_err(|error| fastctx_platform::PublishFailure { error, published: false })?;
                fastctx_platform::publish_checked(&path, &bytes, fastctx_platform::Permissions::Private, true, || {
                    let store = publishing.store();
                    check(&store, &author, &member_key, &owner, opened).map_err(std::io::Error::other)?;
                    Ok(store)
                })
            })
            .await
            .map_err(|error| error.to_string())?;
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
            tell(hub, &account, opened, &record.metadata);
            if let Some(why) = failure {
                return Err(words::durability(version, why));
            }
            if !record.metadata.deleted
                && record.metadata.kind == fastctx_types::sentinel_settings::RECORD_KIND
                && record.metadata.key == fastctx_types::sentinel_settings::RECORD_KEY
            {
                crate::sentinel_home::chosen(hub, &from, &account, &record.body);
            }
            Response::Written { metadata: record.metadata }
        }
    };
    Ok(Told::AccountRecords { response })
}

fn tell(hub: &Hub, account: &AccountName, opened: Timestamp, metadata: &Metadata) {
    let readers: Vec<_> = hub
        .sessions()
        .iter()
        .filter(|(_, session)| session.account_records)
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
