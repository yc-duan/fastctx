//! Registration preserves birth ownership while updating a conversation's running address.

use super::*;
use fastctx_types::ResidentId;

#[allow(clippy::too_many_arguments)]
pub(super) async fn register(
    hub: &Arc<Hub>,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    resident: ResidentId,
    shared: bool,
    previous: Option<MachineName>,
    born_at: Option<jiff::Timestamp>,
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
        // Mapping the canonical identity changes nothing a catalog shows.
        let mut quiet = false;
        let conversation = if let Some(record) = record {
            if record.metadata.deleted {
                return Err("this conversation was deleted".into());
            }
            let mut kept: Conversation = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
            let store = hub.store();
            let directory = &store.book.directory;
            if directory.same(&kept.address.machine, from) {
                // Its host registers it again: the canonical identity is mapped
                // once, from the birth machine's key, when it is still missing.
                let born = kept.previous.first().map_or(Some(*key), |machine| directory.member(machine).map(|m| m.key));
                match (kept.identity.is_none(), born, born_at) {
                    (true, Some(device), Some(born_at)) => {
                        kept.identity = Some(Box::new(identity(&store.hub.id, &device, &resident, born_at)));
                        quiet = true;
                    }
                    _ => return Ok(kept),
                }
            } else {
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
            }
            kept
        } else {
            let source = previous.clone().unwrap_or_else(|| from.clone());
            let store = hub.store();
            let born = shared
                .then(|| {
                    let member = store.book.directory.member(&source)?;
                    Some((member.name.clone(), fastctx_net::record_ownership::seal(member)?))
                })
                .flatten();
            let device = if source == *from { Some(*key) } else { store.book.directory.member(&source).map(|m| m.key) };
            Conversation {
                account: account.clone(),
                address: Address { machine: from.clone(), resident: resident.clone() },
                shared_on: born.as_ref().map(|(machine, _)| machine.clone()),
                born_seal: born.map(|(_, key)| key),
                previous: if source != *from { vec![source] } else { Vec::new() },
                identity: device
                    .zip(born_at)
                    .map(|(device, born_at)| Box::new(identity(&store.hub.id, &device, &resident, born_at))),
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
                if !quiet {
                    super::notify(hub, &conversation);
                }
                return Ok(conversation);
            }
            Told::AccountRecords { response: RecordResponse::Conflict { .. } } => {}
            _ => return Err("conversation registration received an unexpected reply".into()),
        }
    }
}

/// The canonical conversation a host's birth facts name: this hub, the birth
/// machine's device key, the resident and its original birth time.
fn identity(
    hub: &fastctx_types::access::HubId,
    device: &PublicKey,
    resident: &ResidentId,
    born_at: jiff::Timestamp,
) -> fastctx_types::sync::identity::ObjectId {
    fastctx_types::sync::conversation_identity::ConversationKey::Birth {
        hub: hub.clone(),
        device: fastctx_types::sync::identity::DeviceId::from_bytes(*device.as_bytes()),
        resident: resident.clone(),
        born_at,
    }
    .id()
}
