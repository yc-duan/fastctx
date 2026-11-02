//! Resource-level record access keeps the reader separate from the opener.

mod deletion;
pub(crate) mod index;
mod plans;
mod projection;
mod registration;
mod replicas;
mod sentinels;

use crate::{
    Hub,
    account_records::{self, storage},
};
use fastctx_net::{authorization::Action, sign::PublicKey, wire::Told};
use fastctx_types::{
    AccountName, Address, MachineName,
    account_records::{Request as RecordRequest, Response as RecordResponse},
    owned_records::{Conversation, Request, Response},
};
use jiff::Timestamp;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Access {
    pub actor: AccountName,
    pub conversation: Conversation,
}

pub(crate) fn allows(
    directory: &fastctx_net::directory::Directory,
    actor: &AccountName,
    conversation: &Conversation,
) -> bool {
    fastctx_net::record_ownership::allows(
        directory,
        actor,
        std::slice::from_ref(&conversation.account),
        born_on(directory, conversation),
    )
}

/// The machine a conversation was born on, while the member of that name is
/// the very machine it was born on: one that took the name after the first
/// left the network (or was renamed to it) holds none of its records.
pub(crate) fn born_on<'a>(
    directory: &fastctx_net::directory::Directory,
    conversation: &'a Conversation,
) -> Option<&'a MachineName> {
    let machine = conversation.shared_on.as_ref()?;
    born_there(directory, machine, conversation.born_seal.as_deref()).then_some(machine)
}

pub(crate) fn born_there(
    directory: &fastctx_net::directory::Directory,
    machine: &MachineName,
    seal: Option<&str>,
) -> bool {
    seal.is_some_and(|seal| {
        directory.member(machine).and_then(fastctx_net::record_ownership::seal).is_some_and(|now| now == seal)
    })
}

/// A conversation as it is told to others: its birth machine only while
/// that machine is still the one it was born on.
pub(crate) fn verified(directory: &fastctx_net::directory::Directory, conversation: &Conversation) -> Conversation {
    let mut told = conversation.clone();
    if born_on(directory, conversation).is_none() {
        told.shared_on = None;
        told.born_seal = None;
    }
    told
}

pub(crate) fn check(
    hub: &Hub,
    store: &crate::store::Snapshot,
    from: &MachineName,
    key: &PublicKey,
    owner: &AccountName,
    opened: Timestamp,
    access: &Access,
) -> Result<(), String> {
    let directory = &store.book.directory;
    let current = hub
        .account_records
        .owners
        .lock()
        .expect("never poisoned")
        .of_record(owner, opened, fastctx_types::owned_records::KIND, access.conversation.address.resident.as_str())
        .cloned();
    if current.is_none() {
        return Err("this conversation was deleted".into());
    }
    directory.authorize(&access.actor, Action::ActFor(from)).map_err(|error| error.to_string())?;
    if directory.member(from).is_none_or(|member| member.key != *key) {
        return Err("the record reader's device was replaced".into());
    }
    if owner != &access.conversation.account || directory.account(owner).is_none_or(|account| account.opened != opened)
    {
        return Err("the record's owning account was replaced".into());
    }
    if !allows(directory, &access.actor, &access.conversation) {
        return Err("this account no longer owns that conversation's records".into());
    }
    Ok(())
}

pub(crate) fn scoped_body(kind: &str) -> bool {
    matches!(kind, "goal_text" | "goal_history" | "goal_writes" | "sentinel")
}

pub(crate) fn record_key(conversation: &Conversation, kind: &str, key: &str) -> Result<(), String> {
    let resident = conversation.address.resident.as_str();
    let allowed = match kind {
        fastctx_types::plan::KIND | "goal" | "goal_past" => key == resident,
        fastctx_types::history::RECORD_KIND => key
            .strip_prefix(resident)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|name| fastctx_types::history::markdown::check_name(name).is_ok()),
        "goal_text" => key.starts_with("t-"),
        "goal_history" => key.starts_with("h-"),
        "goal_writes" => key.starts_with("w-"),
        "sentinel" => key.parse::<fastctx_types::SentinelId>().is_ok(),
        _ => false,
    };
    if allowed { Ok(()) } else { Err("this key is not a record of that conversation".into()) }
}

fn resolve(hub: &Hub, actor: &AccountName, address: &Address) -> Result<(Access, Timestamp), String> {
    let store = hub.store();
    let directory = &store.book.directory;
    let conversation = hub.account_records.owners.lock().expect("never poisoned").resolve(directory, actor, address)?;
    if !allows(directory, actor, &conversation) {
        return Err("this account does not own that conversation's records".into());
    }
    let opened = directory.account(&conversation.account).ok_or("the record's account is unavailable")?.opened;
    Ok((Access { actor: actor.clone(), conversation }, opened))
}

pub(crate) async fn ask(
    hub: &Arc<Hub>,
    from: MachineName,
    key: PublicKey,
    account: AccountName,
    request: Request,
) -> Result<Told, String> {
    {
        let store = hub.store();
        store.book.directory.authorize(&account, Action::ActFor(&from)).map_err(|error| error.to_string())?;
        if store.book.directory.member(&from).is_none_or(|member| member.key != key) {
            return Err("the record reader's device was replaced".into());
        }
    }
    let mut response = match request {
        Request::Register { resident, shared, previous } => Response::Conversation {
            conversation: registration::register(hub, &from, &key, &account, resident, shared, previous).await?,
        },
        Request::PrepareDeletion { conversation } => {
            let (from, account) = (from.clone(), account.clone());
            hub.blocking(move |hub| crate::forget::prepare(&hub, &from, account, conversation)).await?;
            Response::DeletionPrepared
        }
        Request::Delete { resident } => {
            deletion::delete(hub, &from, &key, &account, &resident).await?;
            Response::Deleted
        }
        Request::Submitted { conversation, kind, key: record_key_name, submission } => {
            if !matches!(kind.as_str(), fastctx_types::plan::KIND | fastctx_types::history::RECORD_KIND) {
                return Err("only plans and history have document revisions".into());
            }
            if submission.len() != 64 || !submission.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("invalid replica submission identity".into());
            }
            let (access, opened) = resolve(hub, &account, &conversation)?;
            record_key(&access.conversation, &kind, &record_key_name)?;
            let slot = hub.account_records.account(&access.conversation.account, opened);
            let records = slot.records.lock().await;
            check(hub, &hub.store(), &from, &key, &access.conversation.account, opened, &access)?;
            let exists = records.entries.contains_key(&(kind.clone(), record_key_name.clone()));
            let (path, history) = (slot.path(&kind, &record_key_name), slot.history(&kind, &record_key_name));
            let record = fastctx_platform::task::spawn_blocking(move || {
                let current = if exists { Some(storage::read(&path)?) } else { None };
                account_records::history::submitted(current.as_ref(), &history, &submission)
            })
            .await
            .map_err(|error| error.to_string())??;
            check(hub, &hub.store(), &from, &key, &access.conversation.account, opened, &access)?;
            Response::Submitted { record }
        }
        Request::Authorize { conversation, reader } => {
            let (access, _) = resolve(hub, &account, &conversation)?;
            Response::Authorization { allowed: allows(&hub.store().book.directory, &reader, &access.conversation) }
        }
        Request::PlanHead { resident, conversation } => {
            plans::head(hub, &from, &key, &account, &resident, conversation.as_ref()).await?
        }
        Request::Sentinels => Response::Sentinels { registrations: sentinels::list(hub, &from, &key, &account).await? },
        Request::Replicas => Response::Replicas { catalog: replicas::catalog(hub, &from, &key, &account).await? },
        Request::List => {
            let store = hub.store();
            let directory = &store.book.directory;
            let conversations = hub
                .account_records
                .owners
                .lock()
                .expect("never poisoned")
                .candidates(directory, &account)
                .into_iter()
                .filter(|conversation| allows(directory, &account, conversation))
                .collect();
            Response::Conversations { conversations }
        }
        Request::Resolve { conversation } => {
            let (access, _) = resolve(hub, &account, &conversation)?;
            Response::Conversation { conversation: access.conversation }
        }
        Request::Record { conversation, request } => {
            let (access, opened) = resolve(hub, &account, &conversation)?;
            match &request {
                RecordRequest::List { kind }
                    if kind == fastctx_types::history::RECORD_KIND
                        || matches!(kind.as_str(), "goal_text" | "goal_history" | "goal_writes" | "sentinel") => {}
                RecordRequest::Read { kind, key }
                | RecordRequest::Write { kind, key, .. }
                | RecordRequest::Delete { kind, key, .. } => record_key(&access.conversation, kind, key)?,
                _ => return Err("this kind of record cannot be listed through a conversation".into()),
            }
            let owner = access.conversation.account.clone();
            match account_records::ask_as(hub, from, key, owner, opened, request, Some(access)).await? {
                Told::AccountRecords { response } => Response::Record { response },
                _ => return Err("the conversation record request received an unexpected reply".into()),
            }
        }
        Request::Catalog { conversation } => {
            let (access, opened) = resolve(hub, &account, &conversation)?;
            let slot = hub.account_records.account(&access.conversation.account, opened);
            let records = slot.records.lock().await;
            check(hub, &hub.store(), &from, &key, &access.conversation.account, opened, &access)?;
            let mut catalog = fastctx_types::history::Catalog {
                documents: records
                    .entries
                    .values()
                    .filter(|entry| record_key(&access.conversation, &entry.metadata.kind, &entry.metadata.key).is_ok())
                    .filter_map(|entry| entry.history.clone())
                    .collect(),
            };
            catalog.sort();
            Response::Catalog { catalog }
        }
        Request::Revision { conversation, kind, key: record_key_name, version } => {
            if !matches!(kind.as_str(), fastctx_types::plan::KIND | fastctx_types::history::RECORD_KIND) {
                return Err("only plans and history have document revisions".into());
            }
            let (access, opened) = resolve(hub, &account, &conversation)?;
            record_key(&access.conversation, &kind, &record_key_name)?;
            let slot = hub.account_records.account(&access.conversation.account, opened);
            let records = slot.records.lock().await;
            check(hub, &hub.store(), &from, &key, &access.conversation.account, opened, &access)?;
            let path = if records
                .entries
                .get(&(kind.clone(), record_key_name.clone()))
                .is_some_and(|entry| entry.metadata.version == version)
            {
                slot.path(&kind, &record_key_name)
            } else {
                slot.history(&kind, &record_key_name).join(format!("{version}.json"))
            };
            let record = fastctx_platform::task::spawn_blocking(move || storage::read(&path))
                .await
                .map_err(|error| error.to_string())??;
            check(hub, &hub.store(), &from, &key, &access.conversation.account, opened, &access)?;
            Response::Revision { record }
        }
        Request::Versions { conversation, kind, key: record_key_name } => {
            if !matches!(kind.as_str(), fastctx_types::plan::KIND | fastctx_types::history::RECORD_KIND) {
                return Err("only plans and history have document revisions".into());
            }
            let (access, opened) = resolve(hub, &account, &conversation)?;
            record_key(&access.conversation, &kind, &record_key_name)?;
            Response::Versions {
                versions: revisions(hub, &from, &key, &access, opened, &kind, &record_key_name).await?,
            }
        }
    };
    projection::redact(hub, &account, &mut response)?;
    Ok(Told::OwnedRecords { response })
}

#[allow(clippy::too_many_arguments)]
async fn revisions(
    hub: &Arc<Hub>,
    from: &MachineName,
    key: &PublicKey,
    access: &Access,
    opened: Timestamp,
    kind: &str,
    name: &str,
) -> Result<Vec<fastctx_types::owned_records::Version>, String> {
    let slot = hub.account_records.account(&access.conversation.account, opened);
    let _gate = slot.records.lock().await;
    check(hub, &hub.store(), from, key, &access.conversation.account, opened, access)?;
    let (path, history) = (slot.path(kind, name), slot.history(kind, name));
    let records = fastctx_platform::task::spawn_blocking(move || {
        let mut records = Vec::new();
        let summary = |path: &std::path::Path| {
            let record = storage::read(path)?;
            let history = storage::history_head(&record).map_err(|error| error.to_string())?;
            Ok::<_, String>(fastctx_types::owned_records::Version { metadata: record.metadata, history })
        };
        match fastctx_platform::disk::metadata(&path) {
            Ok(_) => records.push(summary(&path)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        match fastctx_platform::disk::read_dir(history) {
            Ok(files) => {
                for file in files {
                    let path = file.map_err(|error| error.to_string())?.path();
                    if path.extension().is_some_and(|extension| extension == "json") {
                        records.push(summary(&path)?);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        records.sort_by_key(|record| record.metadata.version);
        records.dedup_by_key(|record| record.metadata.version);
        Ok::<_, String>(records)
    })
    .await
    .map_err(|error| error.to_string())??;
    check(hub, &hub.store(), from, key, &access.conversation.account, opened, access)?;
    Ok(records)
}

/// Rechecked immediately before sending, so queued hints cannot outlive a grant.
pub(crate) fn hint(
    hub: &Hub,
    directory: &fastctx_net::directory::Directory,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    metadata: &fastctx_types::account_records::Metadata,
) -> bool {
    let Some(member) = directory.member(from).filter(|member| member.key == *key) else { return false };
    if directory.authorize(account, Action::ActFor(from)).is_ok() {
        return true;
    }
    if !crate::record_subscriptions::replica_reader(hub, from, key) {
        return false;
    }
    let Some(owner) = directory.account(account) else { return false };
    let owners = hub.account_records.owners.lock().expect("never poisoned");
    let Some(conversation) = owners.of_record(account, owner.opened, &metadata.kind, &metadata.key) else {
        return false;
    };
    member.logins.iter().any(|actor| allows(directory, actor, conversation))
}

/// Legacy account-only plan requests cannot silently create a second co-owned plan.
pub(crate) fn legacy_plan(
    hub: &Hub,
    directory: &fastctx_net::directory::Directory,
    account: &AccountName,
    resident: &str,
) -> Result<(), String> {
    let opened = directory.account(account).ok_or("the account is unavailable")?.opened;
    let owners = hub.account_records.owners.lock().expect("never poisoned");
    if owners.of_record(account, opened, fastctx_types::plan::KIND, resident).is_some() {
        return Ok(());
    }
    if owners.candidates(directory, account).iter().any(|conversation| {
        conversation.address.resident.as_str() == resident && allows(directory, account, conversation)
    }) {
        return Err("update this client to access a shared conversation's plan".into());
    }
    Ok(())
}

/// A birth, move or deletion invalidates catalogs without disclosing the removed identity.
pub(crate) fn notify(hub: &Hub, conversation: &Conversation) {
    let recipients: Vec<_> = {
        let store = hub.store();
        store
            .book
            .directory
            .members
            .iter()
            .filter(|member| member.logins.iter().any(|account| allows(&store.book.directory, account, conversation)))
            .map(|member| member.name.clone())
            .collect()
    };
    for machine in recipients {
        let supported = hub.sessions().get(&machine).is_some_and(|session| session.replicated_records);
        if supported {
            hub.tell(&machine, fastctx_net::wire::Down::ReplicasChanged);
        }
    }
}
