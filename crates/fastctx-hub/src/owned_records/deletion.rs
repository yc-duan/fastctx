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
        tokio::task::spawn_blocking(move || account_records::history::delete(&history))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
