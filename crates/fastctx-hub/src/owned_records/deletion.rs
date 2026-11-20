//! A conversation tombstone blocks new writes before its records and versions are erased.

use super::*;
use fastctx_types::ResidentId;

pub(super) async fn delete(
    hub: &Arc<Hub>,
    from: &MachineName,
    member_key: &PublicKey,
    account: &AccountName,
    resident: &ResidentId,
) -> Result<(), String> {
    let request = RecordRequest::Read { kind: fastctx_types::owned_records::KIND.into(), key: resident.to_string() };
    let opened = account_records::begin(hub, from, member_key, account, &request)?;
    let slot = hub.account_records.account(account, opened);
    loop {
        let Told::AccountRecords { response: RecordResponse::Read { record } } =
            account_records::ask(hub, from.clone(), *member_key, account.clone(), opened, request.clone()).await?
        else {
            return Err("the conversation deletion received an unexpected reply".into());
        };
        let mut notify = None;
        if let Some(record) = &record
            && !record.metadata.deleted
        {
            let conversation: Conversation =
                serde_json::from_value(record.body.clone()).map_err(|error| error.to_string())?;
            notify = Some(conversation.clone());
            if !hub.store().book.directory.same(&conversation.address.machine, from) {
                return Err("delete a conversation through the computer that runs it".into());
            }
            let (from, account, address) = (from.clone(), account.clone(), conversation.address.clone());
            hub.blocking(move |hub| crate::forget::prepare(&hub, &from, account, address)).await?;
        }
        if record.as_ref().is_some_and(|record| record.metadata.deleted) {
            break;
        }
        let deleted = RecordRequest::Delete {
            kind: fastctx_types::owned_records::KIND.into(),
            key: resident.to_string(),
            expected: record.map(|record| record.metadata.version),
        };
        match account_records::ask(hub, from.clone(), *member_key, account.clone(), opened, deleted).await? {
            Told::AccountRecords { response: RecordResponse::Written { .. } } => {
                if let Some(conversation) = &notify {
                    super::notify(hub, conversation);
                }
                break;
            }
            Told::AccountRecords { response: RecordResponse::Conflict { .. } } => {}
            _ => return Err("the conversation deletion received an unexpected reply".into()),
        }
    }
    let documents: Vec<_> = slot
        .records
        .lock()
        .await
        .entries
        .values()
        .filter(|entry| {
            account_records::staged::documents_of(&entry.metadata.kind, &entry.metadata.key, resident.as_str())
        })
        .map(|entry| entry.metadata.clone())
        .collect();
    for metadata in documents {
        if !metadata.deleted {
            let request = RecordRequest::Delete {
                kind: metadata.kind.clone(),
                key: metadata.key.clone(),
                expected: Some(metadata.version),
            };
            match account_records::ask(hub, from.clone(), *member_key, account.clone(), opened, request).await? {
                Told::AccountRecords { response: RecordResponse::Written { .. } } => {}
                _ => return Err("a record changed while its conversation was being deleted".into()),
            }
        }
        let history = slot.history(&metadata.kind, &metadata.key);
        fastctx_platform::task::spawn_blocking(move || account_records::history::delete(&history))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
    }
    // Its own machine asks: what is staged under its id goes.
    staged_gone(&slot, resident, Arc::new(|_| true)).await.map(|_| ())
}

/// Lets go of what of the conversation `resident`'s documents is staged in
/// `slot` and was never published, with a file published of one the index
/// does not keep (`staged::let_go_of`): its text, taken by the asset library
/// or not, is on the disk all the same (codex-d515b194). Under the records'
/// lock, none of them is staged meanwhile.
async fn staged_gone(
    slot: &Arc<account_records::storage::Slot>,
    resident: &ResidentId,
    owns: Owns,
) -> Result<Vec<String>, String> {
    let records = slot.records.lock().await;
    let indexed: std::collections::BTreeSet<(String, String)> = records
        .entries
        .keys()
        .filter(|(kind, key)| account_records::staged::documents_of(kind, key, resident.as_str()))
        .cloned()
        .collect();
    let (held, of) = (slot.clone(), resident.to_string());
    let let_go = fastctx_platform::task::spawn_blocking(move || {
        account_records::staged::let_go_of(&held, &of, &indexed, &*owns)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    drop(records);
    let mut left = Vec::new();
    if let_go.foreign > 0 {
        left.push(format!("{} of its revisions staged that another machine wrote", let_go.foreign));
    }
    if let_go.unread > 0 {
        left.push(format!(
            "{} revisions staged in the account's records that could not be read as any conversation's",
            let_go.unread
        ));
    }
    Ok(left)
}

/// Whether a record of a conversation being let go of is proven its own.
type Owns = Arc<dyn Fn(&fastctx_types::account_records::Metadata) -> bool + Send + Sync>;

/// What erasing a purged conversation's record came to (`erase`).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Erased {
    /// Its record, and its plan and history, are gone.
    Gone,
    /// The hub keeps no record of it, nor ever did under this account as
    /// opened.
    Unrecorded,
    /// The record of its id is another conversation's, or it moved since:
    /// kept, for a person to look at.
    Another,
}

/// Whether `conversation`, a record as read, is the purged conversation's:
/// the one of its id living on the machine `machine` names by its key, born
/// under one of `grants`, never moved (codex-87c8bbd9).
fn its_own(
    hub: &Hub,
    conversation: &Conversation,
    machine: &PublicKey,
    grants: &[fastctx_types::access::GrantId],
) -> bool {
    let here =
        hub.store().book.directory.member(&conversation.address.machine).is_some_and(|member| member.key == *machine);
    let born = [&conversation.birth.born_under, &conversation.birth.claimed_under]
        .into_iter()
        .flatten()
        .any(|grant| grants.contains(grant));
    here && born && conversation.previous.is_empty()
}

/// Whether the record of the conversation `resident` that `account`, as
/// `opened`, opened is the purged conversation's (`Gone`, also where it was
/// deleted already), another's or of one moved since (`Another`), or there
/// is none (`Unrecorded`): read before anything of it on the hub is let go
/// of, which a move would leave the conversation without (codex-87c8bbd9).
pub(crate) async fn judge(
    hub: &Arc<Hub>,
    by: (MachineName, PublicKey),
    account: &AccountName,
    opened: Timestamp,
    resident: &ResidentId,
    grants: &[fastctx_types::access::GrantId],
) -> Result<Erased, String> {
    let request = RecordRequest::Read { kind: fastctx_types::owned_records::KIND.into(), key: resident.to_string() };
    let Told::AccountRecords { response: RecordResponse::Read { record } } =
        account_records::erase(hub, by.clone(), account.clone(), opened, request).await?
    else {
        return Err("the conversation's record was answered with something else".into());
    };
    let Some(record) = record else { return Ok(Erased::Unrecorded) };
    if record.metadata.deleted {
        return Ok(Erased::Gone);
    }
    let conversation: Conversation = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
    Ok(if its_own(hub, &conversation, &by.1, grants) { Erased::Gone } else { Erased::Another })
}

/// Deletes, as the hub itself, the record of the conversation `resident`
/// that `account`, as `opened`, opened, and its plan and history: for a
/// conversation purged, gone where it lived, whose machine `by` may no longer
/// act for the account (`crate::purges`). Each version is let go of only
/// where it is that very conversation's, read again as it is let go of; one
/// deleted already has its plan and history let go of, as no other of its id
/// is recorded under the account after it. Asked again, it goes on.
pub(crate) async fn erase(
    hub: &Arc<Hub>,
    by: (MachineName, PublicKey),
    account: &AccountName,
    opened: Timestamp,
    resident: &ResidentId,
    grants: &[fastctx_types::access::GrantId],
) -> Result<(Erased, Option<String>), String> {
    let request = RecordRequest::Read { kind: fastctx_types::owned_records::KIND.into(), key: resident.to_string() };
    let slot = hub.account_records.account(account, opened);
    let mut recorded = true;
    loop {
        let Told::AccountRecords { response: RecordResponse::Read { record } } =
            account_records::erase(hub, by.clone(), account.clone(), opened, request.clone()).await?
        else {
            return Err("the conversation's erasure received an unexpected reply".into());
        };
        // Never recorded: what the hub keeps under its id is let go of
        // where its machine wrote it, the rest said (codex-d515b194).
        let Some(record) = record else {
            recorded = false;
            break;
        };
        if record.metadata.deleted {
            break;
        }
        let conversation: Conversation =
            serde_json::from_value(record.body.clone()).map_err(|error| error.to_string())?;
        if !its_own(hub, &conversation, &by.1, grants) {
            return Ok((Erased::Another, None));
        }
        let deleted = RecordRequest::Delete {
            kind: fastctx_types::owned_records::KIND.into(),
            key: resident.to_string(),
            expected: Some(record.metadata.version),
        };
        match account_records::erase(hub, by.clone(), account.clone(), opened, deleted).await? {
            Told::AccountRecords { response: RecordResponse::Written { .. } } => {
                super::notify(hub, &conversation);
                break;
            }
            // Changed since it was read: read, and judged, again.
            Told::AccountRecords { response: RecordResponse::Conflict { .. } } => {}
            _ => return Err("the conversation's erasure received an unexpected reply".into()),
        }
    }
    let mut records = slot.records.lock().await;
    // As its disk keeps them, where the hub let go of them since it opened.
    hub.account_records.reopen(account, opened, &slot, &mut records).map_err(|error| error.to_string())?;
    let documents: Vec<_> = records
        .entries
        .values()
        .filter(|entry| {
            account_records::staged::documents_of(&entry.metadata.kind, &entry.metadata.key, resident.as_str())
        })
        .map(|entry| entry.metadata.clone())
        .collect();
    drop(records);
    let store = hub.store();
    let host = by.1;
    let owns: Owns = Arc::new(move |metadata: &fastctx_types::account_records::Metadata| {
        recorded
            || metadata
                .by
                .as_ref()
                .and_then(|machine| store.book.directory.member(machine))
                .is_some_and(|member| member.key == host)
    });
    let (documents, foreign): (Vec<_>, Vec<_>) = documents.into_iter().partition(|metadata| owns(metadata));
    let mut left = Vec::new();
    if !foreign.is_empty() {
        left.push(format!("{} of its documents another machine wrote", foreign.len()));
    }
    for metadata in documents {
        if !metadata.deleted {
            let request = RecordRequest::Delete {
                kind: metadata.kind.clone(),
                key: metadata.key.clone(),
                expected: Some(metadata.version),
            };
            match account_records::erase(hub, by.clone(), account.clone(), opened, request).await? {
                Told::AccountRecords { response: RecordResponse::Written { .. } } => {}
                _ => return Err("a record changed while its conversation was being erased".into()),
            }
        }
        let history = slot.history(&metadata.kind, &metadata.key);
        fastctx_platform::task::spawn_blocking(move || account_records::history::delete(&history))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
    }
    left.extend(staged_gone(&slot, resident, owns).await?);
    let left = (!left.is_empty()).then(|| left.join("; "));
    Ok((if recorded { Erased::Gone } else { Erased::Unrecorded }, left))
}
