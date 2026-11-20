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
            (entry.metadata.kind == fastctx_types::plan::KIND && entry.metadata.key == resident.as_str())
                || (entry.metadata.kind == fastctx_types::history::RECORD_KIND
                    && entry.metadata.key.strip_prefix(resident.as_str()).is_some_and(|suffix| suffix.starts_with('/')))
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
    Ok(())
}

/// Deletes, as the hub itself, the record of the conversation `resident`
/// that `account`, as `opened`, opened, and its plan and history: for a
/// conversation purged, gone where it lived, whose machine `by` may no longer
/// act for the account (`crate::purges`). Asked again of one deleted, it
/// does nothing.
pub(crate) async fn erase(
    hub: &Arc<Hub>,
    by: (MachineName, PublicKey),
    account: &AccountName,
    opened: Timestamp,
    resident: &ResidentId,
) -> Result<(), String> {
    let request = RecordRequest::Read { kind: fastctx_types::owned_records::KIND.into(), key: resident.to_string() };
    let slot = hub.account_records.account(account, opened);
    loop {
        let Told::AccountRecords { response: RecordResponse::Read { record } } =
            account_records::erase(hub, by.clone(), account.clone(), opened, request.clone()).await?
        else {
            return Err("the conversation's erasure received an unexpected reply".into());
        };
        let Some(record) = record.filter(|record| !record.metadata.deleted) else { break };
        let conversation: Conversation =
            serde_json::from_value(record.body.clone()).map_err(|error| error.to_string())?;
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
            Told::AccountRecords { response: RecordResponse::Conflict { .. } } => {}
            _ => return Err("the conversation's erasure received an unexpected reply".into()),
        }
    }
    let documents: Vec<_> = slot
        .records
        .lock()
        .await
        .entries
        .values()
        .filter(|entry| {
            (entry.metadata.kind == fastctx_types::plan::KIND && entry.metadata.key == resident.as_str())
                || (entry.metadata.kind == fastctx_types::history::RECORD_KIND
                    && entry.metadata.key.strip_prefix(resident.as_str()).is_some_and(|suffix| suffix.starts_with('/')))
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
    Ok(())
}

/// Whether the record of the conversation `resident` that `account`, as
/// `opened`, opened is this very conversation's (`crate::purges`): the one of
/// its id living on the machine `by` names by its key, born under one of
/// `grants`. None where there is no record of it now.
pub(crate) async fn verify(
    hub: &Arc<Hub>,
    by: (MachineName, PublicKey),
    account: &AccountName,
    opened: Timestamp,
    resident: &ResidentId,
    grants: &[fastctx_types::access::GrantId],
) -> Result<Option<bool>, String> {
    let request = RecordRequest::Read { kind: fastctx_types::owned_records::KIND.into(), key: resident.to_string() };
    let Told::AccountRecords { response: RecordResponse::Read { record } } =
        account_records::erase(hub, by.clone(), account.clone(), opened, request).await?
    else {
        return Err("the conversation's record was answered with something else".into());
    };
    let Some(record) = record.filter(|record| !record.metadata.deleted) else { return Ok(None) };
    let conversation: Conversation = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
    let here =
        hub.store().book.directory.member(&conversation.address.machine).is_some_and(|member| member.key == by.1);
    let born = [&conversation.birth.born_under, &conversation.birth.claimed_under]
        .into_iter()
        .flatten()
        .any(|grant| grants.contains(grant));
    Ok(Some(here && born && conversation.previous.is_empty()))
}
