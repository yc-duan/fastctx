//! Discover only sentinel registrations attached to conversations the actor owns.

use super::*;
use fastctx_types::sentinel_control::Registration;

pub(super) async fn list(
    hub: &Arc<Hub>,
    from: &MachineName,
    key: &PublicKey,
    actor: &AccountName,
) -> Result<Vec<Registration>, String> {
    let conversations = {
        let store = hub.store();
        hub.account_records
            .owners
            .lock()
            .expect("never poisoned")
            .candidates(&store.book.directory, actor)
            .into_iter()
            .filter(|conversation| allows(&store.book.directory, actor, conversation))
            .collect::<Vec<_>>()
    };
    let mut result = Vec::new();
    for conversation in conversations {
        let (access, opened) = resolve(hub, actor, &conversation.address)?;
        let slot = hub.account_records.account(&conversation.account, opened);
        let records = slot.records.clone().lock_owned().await;
        check(hub, &hub.store(), from, key, &conversation.account, opened, &access)?;
        let paths: Vec<_> = records
            .entries
            .values()
            .filter(|entry| {
                entry.metadata.kind == "sentinel"
                    && entry.conversation.as_deref() == Some(conversation.address.resident.as_str())
            })
            .map(|entry| slot.path("sentinel", &entry.metadata.key))
            .collect();
        let found = fastctx_platform::task::spawn_blocking(move || {
            let _records = records;
            paths
                .into_iter()
                .map(|path| {
                    let record = storage::read(&path)?;
                    let mut registration: Registration =
                        serde_json::from_value(record.body).map_err(|error| error.to_string())?;
                    if record.metadata.deleted {
                        registration.active = false;
                    }
                    Ok::<_, String>(registration)
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .await
        .map_err(|error| error.to_string())??;
        check(hub, &hub.store(), from, key, &conversation.account, opened, &access)?;
        result.extend(found);
    }
    let store = hub.store();
    result.retain(|registration| {
        hub.account_records
            .owners
            .lock()
            .expect("never poisoned")
            .resolve(&store.book.directory, actor, &registration.definition.owner)
            .is_ok()
    });
    Ok(result)
}
