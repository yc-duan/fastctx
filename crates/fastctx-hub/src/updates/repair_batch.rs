//! Repair requests commit bounded chunks after their manifests are prepared.
use crate::{Hub, store::RepairFor};
use fastctx_net::{authorization::Action, directory_index::Indexed};
use fastctx_types::{
    AccountName, MachineName,
    update::{Manifest, RepairDue},
};
use std::sync::Arc;

pub(super) type Prepared = (usize, MachineName, Option<Arc<Manifest>>);

pub(super) fn apply(
    hub: &Hub,
    asker: &MachineName,
    account: &AccountName,
    items: &[Prepared],
) -> Vec<Result<(), String>> {
    let mut store = hub.store();
    let index = hub.index_of(&store);
    let before = store.hub.repairs.clone();
    let mut results = Vec::with_capacity(items.len());
    for (_, machine, manifest) in items {
        let result = (|| {
            let directory = Indexed::new(&store.book.directory, &index);
            if !directory.logged_in(asker, account) || directory.account(account).is_none_or(|a| a.locked) {
                return Err(format!("{asker} is not logged in to an unlocked account {account}"));
            }
            if directory.authorize(account, Action::Manage(machine)).is_err() {
                return Err("this account can no longer manage that machine".into());
            }
            let key = directory.member(machine).ok_or("repair target no longer exists")?.key;
            store.hub.repairs.retain(|r| r.key != key);
            if let Some(manifest) = manifest {
                let at = jiff::Timestamp::now();
                let id = format!("{}-{}", store.hub.id, at.as_nanosecond());
                store.hub.repairs.push(Arc::new(RepairFor {
                    key,
                    due: RepairDue {
                        id,
                        version: manifest.version.clone(),
                        by: account.clone(),
                        at,
                        manifest: Some((**manifest).clone()),
                        problem: None,
                        attempts: 0,
                        retry_at: None,
                        observed: None,
                    },
                }));
            }
            Ok(())
        })();
        results.push(result);
    }
    if results.iter().any(Result::is_ok)
        && let Err(error) = store.save_hub()
    {
        store.hub.repairs = before;
        return items
            .iter()
            .map(|(_, machine, _)| Err(format!("saving repair arrangement for {machine}: {error}")))
            .collect();
    }
    drop(store);
    for ((_, machine, manifest), result) in items.iter().zip(&results) {
        if result.is_ok() {
            hub.log.line(format_args!(
                "{account} {} repair for {machine}",
                if manifest.is_none() { "cancelled" } else { "arranged" }
            ));
        }
    }
    results
}
