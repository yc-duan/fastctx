//! Registration preserves birth ownership while updating a conversation's running address.

use super::*;
use fastctx_types::ResidentId;

pub(super) async fn register(
    hub: &Arc<Hub>,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    resident: ResidentId,
    shared: bool,
    previous: Option<MachineName>,
) -> Result<Conversation, String> {
    let name = resident.to_string();
    let read = RecordRequest::Read { kind: fastctx_types::owned_records::KIND.into(), key: name.clone() };
    let opened = account_records::begin(hub, from, key, account, &read)?;
    loop {
        let Told::AccountRecords { response: RecordResponse::Read { record } } =
            account_records::ask(hub, from.clone(), *key, account.clone(), opened, read.clone()).await?
        else {
            return Err("conversation registration received an unexpected reply".into());
        };
        let expected = record.as_ref().map(|record| record.metadata.version);
        let conversation = if let Some(record) = record {
            if record.metadata.deleted {
                return Err("this conversation was deleted".into());
            }
            let mut kept: Conversation = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
            let directory = &hub.store().book.directory;
            if directory.same(&kept.address.machine, from) {
                return Ok(kept);
            }
            if previous.as_ref().is_none_or(|source| {
                !directory.same(source, &kept.address.machine)
                    && !kept.previous.iter().any(|old| directory.same(source, old))
            }) {
                return Err("the conversation's previous address does not match its registered owner".into());
            }
            if !kept.previous.contains(&kept.address.machine) {
                kept.previous.push(kept.address.machine.clone());
            }
            kept.address.machine = from.clone();
            kept
        } else {
            let source = previous.clone().unwrap_or_else(|| from.clone());
            Conversation {
                account: account.clone(),
                address: Address { machine: from.clone(), resident: resident.clone() },
                shared_on: shared.then_some(source.clone()),
                previous: if source != *from { vec![source] } else { Vec::new() },
            }
        };
        let request = RecordRequest::Write {
            kind: fastctx_types::owned_records::KIND.into(),
            key: name.clone(),
            expected,
            body: serde_json::to_value(&conversation).map_err(|error| error.to_string())?,
        };
        match account_records::ask(hub, from.clone(), *key, account.clone(), opened, request).await? {
            Told::AccountRecords { response: RecordResponse::Written { .. } } => {
                super::notify(hub, &conversation);
                return Ok(conversation);
            }
            Told::AccountRecords { response: RecordResponse::Conflict { .. } } => {}
            _ => return Err("conversation registration received an unexpected reply".into()),
        }
    }
}
