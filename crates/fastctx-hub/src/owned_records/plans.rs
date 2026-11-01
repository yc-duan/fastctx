//! A plan watch resolves ownership once and retains that conversation across invalidations.

use super::*;
use fastctx_types::ResidentId;

pub(super) async fn head(
    hub: &Arc<Hub>,
    from: &MachineName,
    key: &PublicKey,
    actor: &AccountName,
    resident: &ResidentId,
    pinned: Option<&Address>,
) -> Result<Response, String> {
    let address = match pinned {
        Some(address) => Some(address.clone()),
        None => {
            let store = hub.store();
            let mut choices = hub
                .account_records
                .owners
                .lock()
                .expect("never poisoned")
                .candidates(&store.book.directory, actor)
                .into_iter()
                .filter(|conversation| {
                    conversation.address.resident == *resident && allows(&store.book.directory, actor, conversation)
                });
            let found = choices.next();
            if choices.next().is_some() {
                return Err("the plan name identifies more than one visible conversation".into());
            }
            found.map(|conversation| conversation.address)
        }
    };
    let (access, opened, account) = match address {
        Some(address) => {
            let (access, opened) = resolve(hub, actor, &address)?;
            let account = access.conversation.account.clone();
            (Some(access), opened, account)
        }
        None => {
            let store = hub.store();
            let opened = store.book.directory.account(actor).ok_or("the account is unavailable")?.opened;
            (None, opened, actor.clone())
        }
    };
    let slot = hub.account_records.account(&account, opened);
    let records = slot.records.lock().await;
    if let Some(access) = &access {
        check(hub, &hub.store(), from, key, &account, opened, access)?;
    } else {
        account_records::authorized(hub, from, key, actor, opened)?;
    }
    crate::record_subscriptions::follow(hub, from, key, &account, fastctx_types::plan::KIND, resident.as_str())?;
    let metadata = records
        .entries
        .get(&(fastctx_types::plan::KIND.into(), resident.to_string()))
        .map(|entry| entry.metadata.clone());
    Ok(Response::PlanHead { conversation: access.map(|access| access.conversation), metadata })
}
