//! Account records are independent of the directory. Each account incarnation
//! serializes its own operations; disk work never holds the registry lock.

pub(crate) mod history;
mod history_submission;
pub(crate) mod revisions;
pub(crate) mod staged;
pub(crate) mod storage;

use std::sync::Arc;

use fastctx_net::authorization::Action;
use fastctx_net::sign::PublicKey;
use fastctx_net::wire::{Down, Told};
use fastctx_types::account_records::{Changed, MAX_KIND_KEYS, Metadata, Record, Request, Response, words};
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
    store: &crate::store::Snapshot,
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
    if let Request::Read { kind, key } | Request::Write { kind, key, .. } | Request::Delete { kind, key, .. } = request
        && kind == fastctx_types::plan::KIND
    {
        crate::owned_records::legacy_plan(hub, directory, account, key)?;
    }
    Ok(opened)
}

/// Routes record deletions that erase every historical version to durable work.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn asked(
    hub: &Arc<Hub>,
    from: MachineName,
    member_key: PublicKey,
    account: AccountName,
    opened: Timestamp,
    request: Request,
    named: Option<fastctx_types::submission::Submission>,
) -> Result<Told, String> {
    if matches!(&request, Request::Delete { kind, .. } if crate::owned_records::record_deletions::versioned(kind)) {
        crate::owned_records::record_deletions::account(hub, from, member_key, account, opened, request, named.as_ref())
            .await
    } else {
        ask(hub, from, member_key, account, opened, request).await
    }
}

pub(crate) async fn ask(
    hub: &Arc<Hub>,
    from: MachineName,
    member_key: PublicKey,
    account: AccountName,
    opened: Timestamp,
    request: Request,
) -> Result<Told, String> {
    ask_as(hub, from, member_key, account, opened, request, None).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn ask_as(
    hub: &Arc<Hub>,
    from: MachineName,
    member_key: PublicKey,
    account: AccountName,
    opened: Timestamp,
    request: Request,
    access: Option<crate::owned_records::Access>,
) -> Result<Told, String> {
    asked_of(hub, from, member_key, account, opened, request, access, false).await
}

/// Deletes, as the hub itself, the record `kind` `key` of `account` as
/// `opened`, at `expected`: for what the hub lets go of itself, as of a
/// conversation purged, whose machine may no longer act for the account
/// (`crate::purges`). `by` is the machine the record is said deleted by.
pub(crate) async fn erase(
    hub: &Arc<Hub>,
    by: (MachineName, PublicKey),
    account: AccountName,
    opened: Timestamp,
    request: Request,
) -> Result<Told, String> {
    if !matches!(&request, Request::Read { .. } | Request::Delete { .. }) {
        return Err("the hub only reads and deletes records itself".into());
    }
    asked_of(hub, by.0, by.1, account, opened, request, None, true).await
}

#[allow(clippy::too_many_arguments)]
async fn asked_of(
    hub: &Arc<Hub>,
    from: MachineName,
    member_key: PublicKey,
    account: AccountName,
    opened: Timestamp,
    request: Request,
    access: Option<crate::owned_records::Access>,
    by_hub: bool,
) -> Result<Told, String> {
    request.validate()?;
    let slot = hub.account_records.account(&account, opened);
    let mut records = slot.records.clone().lock_owned().await;
    let hub = hub.clone();
    crate::readers::blocking(move || {
        if !by_hub {
            permitted(&hub, &hub.store(), &from, &member_key, &account, opened, access.as_ref())?;
        }
        for metadata in crate::skill_storage::recover(&slot, &mut records)? {
            tell(&hub, &account, opened, &metadata);
        }
        let response = match request {
            Request::List { kind } => Response::Listed {
                entries: records
                    .entries
                    .values()
                    .filter(|entry| entry.metadata.kind == kind && !entry.metadata.deleted)
                    .filter(|entry| {
                        access.as_ref().is_none_or(|access| {
                            crate::owned_records::record_key(
                                &access.conversation,
                                &entry.metadata.kind,
                                &entry.metadata.key,
                            )
                            .is_ok()
                                && (!crate::owned_records::scoped_body(&entry.metadata.kind)
                                    || entry.conversation.as_deref()
                                        == Some(access.conversation.address.resident.as_str()))
                        })
                    })
                    .map(|entry| entry.metadata.clone())
                    .collect(),
            },
            Request::Read { kind, key } => {
                if !by_hub {
                    crate::record_subscriptions::follow(&hub, &from, &member_key, &account, &kind, &key)?;
                }
                if let Some(access) = &access
                    && crate::owned_records::scoped_body(&kind)
                    && records.entries.get(&(kind.clone(), key.clone())).is_some_and(|entry| {
                        !entry.metadata.deleted
                            && entry.conversation.as_deref() != Some(access.conversation.address.resident.as_str())
                    })
                {
                    return Err("this record belongs to another conversation".into());
                }

                let probe = probe(&slot, &records, &kind, &key, Value::Null)?;
                caught_up(&hub, &slot, &mut records, &account, opened, &probe)?;
                let record = if records.entries.contains_key(&(kind.clone(), key.clone())) {
                    let path = slot.path(&kind, &key);
                    Some(storage::read(&path)?)
                } else {
                    None
                };
                if !by_hub {
                    permitted(&hub, &hub.store(), &from, &member_key, &account, opened, access.as_ref())?;
                }
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
                let parent = match kind.as_str() {
                    fastctx_types::history::RECORD_KIND => key.split_once('/').map(|(resident, _)| resident),
                    fastctx_types::plan::KIND | "goal" | "goal_past" => Some(key.as_str()),
                    _ => None,
                };
                let parent_deleted = parent.is_some_and(|resident| {
                    records
                        .entries
                        .get(&(fastctx_types::owned_records::KIND.into(), resident.into()))
                        .is_some_and(|entry| entry.metadata.deleted)
                });
                if parent_deleted && !deleted {
                    return Err("this conversation was deleted".into());
                }
                if kind == fastctx_types::history::RECORD_KIND && deleted && !parent_deleted {
                    return Err("history documents are retired as a new revision; their versions are preserved".into());
                }
                // A revision the asset library took whose file was not
                // published is published first: what is checked against is
                // what was accepted.
                let probe = probe(&slot, &records, &kind, &key, if deleted { Value::Null } else { body.clone() })?;
                caught_up(&hub, &slot, &mut records, &account, opened, &probe)?;
                let identity = (kind.clone(), key.clone());
                let before = records.entries.get(&identity);
                if let Some(access) = &access
                    && crate::owned_records::scoped_body(&kind)
                {
                    if before.is_some_and(|entry| {
                        !entry.metadata.deleted
                            && entry.conversation.as_deref() != Some(access.conversation.address.resident.as_str())
                    }) {
                        return Err("this record belongs to another conversation".into());
                    }
                    if !deleted {
                        let resident = if kind == "sentinel" {
                            body["definition"]["owner"]["resident"].as_str()
                        } else {
                            body["resident"].as_str()
                        };
                        if resident != Some(access.conversation.address.resident.as_str()) {
                            return Err("the new record belongs to another conversation".into());
                        }
                    }
                }
                if let Some(access) = &access
                    && !deleted
                    && matches!(kind.as_str(), "goal" | "goal_past")
                {
                    if let Some(resident) = body.get("resident").filter(|value| !value.is_null())
                        && resident["resident"].as_str() != Some(access.conversation.address.resident.as_str())
                    {
                        return Err("the new goal belongs to another conversation".into());
                    }
                    if kind == "goal" && body["account"]["account"].as_str() != Some(account.as_str()) {
                        return Err("the new goal belongs to another account".into());
                    }
                }
                if kind == "sentinel" && deleted && before.is_some() {
                    body = storage::read(&slot.path(&kind, &key))?.body;
                }
                if let Some(access) = &access
                    && kind == "sentinel"
                    && !deleted
                {
                    let registration: fastctx_types::sentinel_control::Registration =
                        serde_json::from_value(body.clone()).map_err(|error| error.to_string())?;
                    if registration.scope.account != account
                        || registration.definition.account != account
                        || registration.definition.id.as_str() != key
                        || registration.definition.owner.resident != access.conversation.address.resident
                    {
                        return Err("the sentinel belongs to another account or conversation".into());
                    }
                    if let Some(before) = before.filter(|entry| !entry.metadata.deleted) {
                        let previous: fastctx_types::sentinel_control::Registration =
                            serde_json::from_value(storage::read(&slot.path(&kind, &key))?.body)
                                .map_err(|error| error.to_string())?;
                        if previous.scope != registration.scope || previous.definition != registration.definition {
                            return Err("shared sentinel controls cannot change its assignment".into());
                        }
                        let _ = before;
                    }
                }
                if kind == fastctx_types::history::RECORD_KIND
                    && !deleted
                    && let Some(metadata) = history_submission::accepted(&slot, &records, &key, &body)?
                {
                    permitted(&hub, &hub.store(), &from, &member_key, &account, opened, access.as_ref())?;
                    return Ok(Told::AccountRecords { response: Response::Written { metadata } });
                }
                let version = before.map(|entry| entry.metadata.version);
                if expected != version {
                    return Ok(Told::AccountRecords {
                        response: Response::Conflict { current: before.map(|entry| entry.metadata.clone()) },
                    });
                }
                if kind == fastctx_types::owned_records::KIND && !deleted {
                    let ownership: fastctx_types::owned_records::Conversation =
                        serde_json::from_value(body.clone()).map_err(|error| error.to_string())?;
                    let store = hub.store();
                    if store.book.directory.member(&from).is_none_or(|member| !member.is_machine() || member.managed)
                        || ownership.account != account
                        || ownership.address.resident.as_str() != key
                        || !store.book.directory.same(&ownership.address.machine, &from)
                    {
                        return Err("a host registers only its own conversation for its acting account".into());
                    }
                    // What it was born as is kept once, whole and as it
                    // holds, however the write comes (`owned_records::birth_kept`).
                    if before.is_none() {
                        crate::owned_records::birth_kept(&store.book.directory, None, &ownership)?;
                        // Born under an assistance ended for good, it is being
                        // deleted where it was born: it is not registered anew
                        // (`purges`).
                        let closed = crate::purges::closed(&store.book.purges);
                        if [&ownership.birth.born_under, &ownership.birth.claimed_under]
                            .into_iter()
                            .flatten()
                            .any(|grant| closed.contains(grant))
                        {
                            return Err("it was born under an assistance ended for good".into());
                        }
                    }
                    if let Some(before) = before {
                        if before.metadata.deleted {
                            return Err("this conversation was deleted".into());
                        }
                        let old: fastctx_types::owned_records::Conversation =
                            serde_json::from_value(storage::read(&slot.path(&kind, &key))?.body)
                                .map_err(|error| error.to_string())?;
                        crate::owned_records::birth_kept(&store.book.directory, Some(&old), &ownership)?;
                        // Its earlier addresses are what a move makes of them
                        // (`registration`): as they were where it stays, with
                        // the one it leaves added after them where it moves;
                        // where it was born is never written anew (codex-87c8bbd9).
                        let moved = !store.book.directory.same(&ownership.address.machine, &old.address.machine);
                        let mut previous = old.previous.clone();
                        if moved && !previous.contains(&old.address.machine) {
                            previous.push(old.address.machine.clone());
                        }
                        if old.account != ownership.account
                            || old.shared_on != ownership.shared_on
                            || old.born_seal != ownership.born_seal
                            || ownership.previous != previous
                        {
                            return Err("a conversation's birth ownership cannot be rewritten".into());
                        }
                    } else if let Some(source) = &ownership.shared_on {
                        store
                            .book
                            .directory
                            .authorize(&account, Action::Work(source))
                            .map_err(|error| error.to_string())?;
                        // Born on that very machine: its key, not a name another may take.
                        if !crate::owned_records::born_there(
                            &store.book.directory,
                            source,
                            ownership.born_seal.as_deref(),
                        ) {
                            return Err("a shared birth names the key of the machine it was born on".into());
                        }
                    }
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
                    && !matches!(
                        kind.as_str(),
                        fastctx_types::skills::KIND
                            | fastctx_types::history::RECORD_KIND
                            | fastctx_types::owned_records::KIND
                            | fastctx_types::plan::KIND
                    )
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
                // Above every version written before the hub was last put
                // back: a write on one the restore lost answers "changed"
                // (§29, 2.2).
                let version = before
                    .map_or(0, |entry| entry.metadata.version)
                    .checked_add(1)
                    .ok_or(words::REVISION)?
                    .max(hub.assets.number_floor());
                let mut history_author = None;
                if kind == fastctx_types::history::RECORD_KIND && !deleted {
                    let stored: fastctx_types::history::stored::Stored =
                        serde_json::from_value(body.clone()).map_err(|error| error.to_string())?;
                    stored.validate()?;
                    crate::history_author::check(
                        &hub,
                        &hub.store(),
                        &from,
                        &member_key,
                        access.as_ref().map_or(&account, |access| &access.actor),
                        &stored.revision.author,
                        stored.submission.is_some(),
                    )?;
                    history_author = Some((stored.revision.author.clone(), stored.submission.is_some()));
                    fastctx_types::history::markdown::parse_written(&stored.revision.name, &stored.text)?;
                    crate::history_limits::conversation(&records, &key)?;
                    if stored.revision.version != version
                        || key.rsplit('/').next() != Some(stored.revision.name.as_str())
                    {
                        return Err("the history revision does not match its record key and version".into());
                    }
                }
                let record = Record {
                    metadata: Metadata { kind, key, version, by: Some(from.clone()), at: Timestamp::now(), deleted },
                    body,
                };
                let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
                let size = bytes.len();
                let total = records.bytes - before.map_or(0, |entry| entry.size) + size;
                let specialized = record.metadata.kind == fastctx_types::skills::KIND
                    || record.metadata.kind == fastctx_types::skills::INDEX_KIND;
                // What a revision can fill is the hub's disk: one that grows a
                // record leaves room to preserve the one before and publish it.
                if size > before.map_or(0, |entry| entry.size) {
                    let available =
                        fastctx_platform::space::available(slot.root()).map_err(|error| error.to_string())?;
                    if (size as u64).saturating_mul(2) > available {
                        return Err("there is not enough disk space to preserve and publish this revision".into());
                    }
                }
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
                let path = slot.path(&record.metadata.kind, &record.metadata.key);
                let archive = staged::archived(&record.metadata.kind);
                let historical = archive.then(|| slot.history(&record.metadata.kind, &record.metadata.key));
                // A record with a place in the asset library goes there first,
                // as its truth; the file is its projection (design §15).
                let place = crate::assets::mirror::place_of(&hub.assets, &account, opened, &record)?;
                let first = place.is_some();
                let mut projected = true;
                let failure = if let Some(place) = place {
                    if !by_hub {
                        permitted(&hub, &hub.store(), &from, &member_key, &account, opened, access.as_ref())?;
                    }
                    if let Some((history_author, submitted)) = &history_author {
                        crate::history_author::check(
                            &hub,
                            &hub.store(),
                            &from,
                            &member_key,
                            access.as_ref().map_or(&account, |access| &access.actor),
                            history_author,
                            *submitted,
                        )?;
                    }
                    let staging = staged::path(&slot, &record.metadata.kind, &record.metadata.key);
                    staged::stage(&staging, &bytes).map_err(words::store)?;
                    match crate::assets::mirror::take_first(
                        &hub.assets,
                        std::slice::from_ref(&place),
                        std::slice::from_ref(&record),
                    ) {
                        Ok(()) => {}
                        // Refused before anything was written: nothing of it
                        // stands. An earlier write of it given up on as its
                        // answer did not come may have been taken since —
                        // the library's writer goes on with it — and its staged
                        // revision written over by this one: what the library
                        // holds is published, so neither the file nor the
                        // index is left behind it (codex-357de52a, fourth
                        // round #9).
                        Err(crate::assets::mirror::NotTaken::Refused(why)) => {
                            let _ = fastctx_platform::disk::remove_file(&staging);
                            if let Err(error) = caught_up(&hub, &slot, &mut records, &account, opened, &record) {
                                hub.log.line(format_args!(
                                    "publishing {} {} from the asset library after a write was refused: {error}",
                                    record.metadata.kind, record.metadata.key
                                ));
                            }
                            return Err(why);
                        }
                        // Failed as it wrote: taken if the library holds this
                        // very revision; else not known — a write may still be
                        // finishing — and the staged revision stays for whoever
                        // reads it next, or the next opening (codex-357de52a #6).
                        Err(crate::assets::mirror::NotTaken::Unknown(why)) => {
                            if !crate::assets::mirror::holds_revision(&hub.assets, &account, opened, &record)
                                .unwrap_or(false)
                            {
                                return Err(why);
                            }
                        }
                    }
                    // Taken: the write stands. A file not published now is
                    // published from the library as the record is next asked
                    // for (`caught_up`), or from its staged revision as the hub
                    // opens; till then the index stays with the file.
                    if let Err(error) = staged::publish(&path, historical.as_deref(), &staging, &bytes, &record) {
                        projected = false;
                        hub.log.line(format_args!(
                            "publishing {} {} after the asset library took it: {error}",
                            record.metadata.kind, record.metadata.key
                        ));
                    }
                    None
                } else {
                    legacy_publish(
                        &hub,
                        &from,
                        &member_key,
                        &account,
                        opened,
                        access.as_ref(),
                        history_author.as_ref(),
                        &path,
                        historical,
                        &bytes,
                        deleted,
                        by_hub,
                    )?
                };
                if record.metadata.kind == fastctx_types::owned_records::KIND {
                    let ownership = if record.metadata.deleted {
                        None
                    } else {
                        Some(serde_json::from_value(record.body.clone()).map_err(|error| error.to_string())?)
                    };
                    hub.account_records.owners.lock().expect("never poisoned").take(
                        &account,
                        opened,
                        &record.metadata.key,
                        ownership,
                    );
                }
                if projected {
                    records.bytes = total;
                    records.entries.insert(
                        identity,
                        storage::Entry {
                            metadata: record.metadata.clone(),
                            size,
                            history: storage::history_head(&record).map_err(|error| error.to_string())?,
                            conversation: storage::record_conversation(&record),
                        },
                    );
                    slot.count.store(records.entries.len(), std::sync::atomic::Ordering::Relaxed);
                    slot.bytes.store(total, std::sync::atomic::Ordering::Relaxed);
                }
                // Keep the account gate until hints are queued, preserving revision order.
                if first {
                    hint(&hub, &account, opened, &record.metadata);
                } else {
                    tell(&hub, &account, opened, &record.metadata);
                }
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

/// What finds a record's place in the asset library: the file the hub keeps
/// of it where there is one, otherwise `body` under its kind and key.
fn probe(
    slot: &storage::Slot,
    records: &storage::Account,
    kind: &str,
    key: &str,
    body: Value,
) -> Result<Record, String> {
    if records.entries.contains_key(&(kind.to_string(), key.to_string())) && body.is_null() {
        return storage::read(&slot.path(kind, key));
    }
    // A record the library took whose file was never published: its staged
    // revision says where it is, as a sentinel's body names its conversation
    // alone (codex-357de52a #4).
    if body.is_null() {
        let staging = staged::path(slot, kind, key);
        if fastctx_platform::disk::is_file(&staging)
            && let Ok(record) = storage::read(&staging)
        {
            return Ok(record);
        }
    }
    Ok(Record {
        metadata: Metadata {
            kind: kind.into(),
            key: key.into(),
            version: 0,
            by: None,
            at: Timestamp::now(),
            deleted: false,
        },
        body,
    })
}

/// Publishes the revision the asset library holds of `probe`'s record where
/// the hub's file of it is behind — one the library took whose file was not
/// published — so the record is read and checked as it was accepted. Call
/// with the account's records held.
fn caught_up(
    hub: &Hub,
    slot: &storage::Slot,
    records: &mut storage::Account,
    account: &AccountName,
    opened: Timestamp,
    probe: &Record,
) -> Result<(), String> {
    if !crate::assets::mirror::mirrored(&probe.metadata.kind) {
        return Ok(());
    }
    let Some(held) = crate::assets::mirror::held_record(&hub.assets, account, opened, probe)? else { return Ok(()) };
    let identity = (held.metadata.kind.clone(), held.metadata.key.clone());
    if identity != (probe.metadata.kind.clone(), probe.metadata.key.clone()) {
        return Err("the asset library holds another record at this one's place".into());
    }
    if records.entries.get(&identity).is_some_and(|entry| entry.metadata.version >= held.metadata.version) {
        return Ok(());
    }
    let bytes = serde_json::to_vec(&held).map_err(|error| error.to_string())?;
    let (kind, key) = (&held.metadata.kind, &held.metadata.key);
    let staging = staged::path(slot, kind, key);
    staged::stage(&staging, &bytes).map_err(words::store)?;
    let history = staged::archived(kind).then(|| slot.history(kind, key));
    staged::publish(&slot.path(kind, key), history.as_deref(), &staging, &bytes, &held).map_err(words::store)?;
    let before = records.entries.get(&identity).map_or(0, |entry| entry.size);
    records.bytes = records.bytes - before + bytes.len();
    records.entries.insert(
        identity,
        storage::Entry {
            metadata: held.metadata.clone(),
            size: bytes.len(),
            history: storage::history_head(&held).map_err(|error| error.to_string())?,
            conversation: storage::record_conversation(&held),
        },
    );
    slot.count.store(records.entries.len(), std::sync::atomic::Ordering::Relaxed);
    slot.bytes.store(records.bytes, std::sync::atomic::Ordering::Relaxed);
    hub.log.line(format_args!(
        "published {kind} {key} at revision {} from the asset library, which took it before its file was",
        held.metadata.version
    ));
    Ok(())
}

/// Publishes a record that has no place in the asset library yet as the
/// hub kept every record before it: the file is checked and published under
/// the directory, and is what the library is written from once the scope
/// is described. Says why it failed after the file was published.
#[allow(clippy::too_many_arguments)]
fn legacy_publish(
    hub: &Arc<Hub>,
    from: &MachineName,
    member_key: &PublicKey,
    account: &AccountName,
    opened: Timestamp,
    access: Option<&crate::owned_records::Access>,
    history_author: Option<&(fastctx_types::history::Author, bool)>,
    path: &std::path::Path,
    historical: Option<std::path::PathBuf>,
    bytes: &[u8],
    deleted: bool,
    by_hub: bool,
) -> Result<Option<String>, String> {
    let result = (|| {
        fastctx_platform::ensure_private_dir(path.parent().expect("record folder"))
            .map_err(|error| fastctx_platform::PublishFailure { error, published: false })?;
        if let Some(history) = &historical {
            history::preserve(path, history)
                .map_err(|error| fastctx_platform::PublishFailure { error, published: false })?;
        }
        let published =
            fastctx_platform::publish_checked(path, bytes, fastctx_platform::Permissions::Private, true, || {
                let store = hub.edit_store();
                if !by_hub {
                    permitted(hub, &store, from, member_key, account, opened, access).map_err(std::io::Error::other)?;
                }
                if let Some((history_author, submitted)) = history_author {
                    crate::history_author::check(
                        hub,
                        &store,
                        from,
                        member_key,
                        access.map_or(account, |access| &access.actor),
                        history_author,
                        *submitted,
                    )
                    .map_err(std::io::Error::other)?;
                }
                Ok(store)
            });
        if deleted
            && (published.is_ok() || published.as_ref().is_err_and(|error| error.published))
            && let Some(history) = &historical
        {
            history::delete(history).map_err(|error| fastctx_platform::PublishFailure { error, published: true })?;
        }
        published
    })();
    match result {
        Ok(()) => Ok(None),
        Err(failure) if failure.published => Ok(Some(failure.error.to_string())),
        Err(failure) => Err(words::store(failure.error)),
    }
}

/// Tells of a record written as the hub kept them before its scope was
/// described: written into the asset library where it now has a place, and
/// hinted to the members who follow it.
pub(crate) fn tell(hub: &Hub, account: &AccountName, opened: Timestamp, metadata: &Metadata) {
    // What of an account and its conversations goes into the asset library
    // goes there too (design §28.17).
    if crate::assets::mirror::mirrored(&metadata.kind) {
        let path = hub.account_records.account(account, opened).path(&metadata.kind, &metadata.key);
        if let Err(error) = storage::read(&path)
            .and_then(|record| crate::assets::mirror::document(&hub.assets, account, opened, &record))
        {
            hub.log.line(format_args!("writing {} {} into the asset library: {error}", metadata.kind, metadata.key));
            crate::assets::mirror::failed(account, opened, path);
        }
    }
    hint(hub, account, opened, metadata);
}

/// Hints a written record to the members who follow it.
pub(crate) fn hint(hub: &Hub, account: &AccountName, opened: Timestamp, metadata: &Metadata) {
    let store = hub.store();
    let directory = store.indexed();
    if directory.account(account).is_none_or(|owner| owner.opened != opened) {
        return;
    }
    let conversation = hub
        .account_records
        .owners
        .lock()
        .expect("never poisoned")
        .of_record(account, opened, &metadata.kind, &metadata.key)
        .cloned();
    let mut candidates = directory.acting_members(account);
    if let Some(conversation) = &conversation {
        candidates.extend(directory.record_readers(
            std::slice::from_ref(&conversation.account),
            crate::owned_records::born_on(&directory, conversation),
        ));
    }
    let readers: Vec<_> = {
        let sessions = hub.sessions();
        fastctx_net::directory_index::ordered(candidates)
            .into_iter()
            .filter_map(|at| {
                let name = &directory.all.members[at].name;
                let session = sessions.get(name)?;
                (session.account_records
                    && session.record_keys.as_ref().is_none_or(|keys| keys.wants(account, metadata)))
                .then(|| (name.clone(), session.key, session.out.clone()))
            })
            .collect()
    };
    for (reader, key, out) in readers {
        if crate::owned_records::hint(hub, &directory, &reader, &key, account, metadata) {
            let changed = Changed { account: account.clone(), metadata: metadata.clone() };
            let _ = out.send(crate::Out::Frame(Down::AccountRecordChanged { changed }));
        }
    }
}

fn permitted(
    hub: &Hub,
    store: &crate::store::Snapshot,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    opened: Timestamp,
    access: Option<&crate::owned_records::Access>,
) -> Result<(), String> {
    match access {
        Some(access) => crate::owned_records::check(hub, store, from, key, account, opened, access),
        None => check(store, from, key, account, opened),
    }
}
