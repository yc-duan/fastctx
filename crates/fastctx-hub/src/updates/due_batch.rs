//! A bounded commit of update arrangements and their individual audit records.
//! Authority is checked again under the store lock, using the ordinary policy.
use crate::{Hub, store::DueFor};
use fastctx_net::{authorization::Action as Authority, directory_index::Indexed};
use fastctx_types::{
    AccountName, MachineName,
    machine_log::{Action, Actor, Outcome},
    update::{Due, FailureReason, Problem},
};
use jiff::Timestamp;
use std::{collections::BTreeMap, sync::Arc};

/// Commits one chunk. A failed transaction fails all items in this chunk;
/// earlier chunks remain committed. Repeated input machines keep their order
/// and each has its own audit record, with the last arrangement taking effect.
pub(super) fn apply(
    hub: &Hub,
    asker: &MachineName,
    account: &AccountName,
    machines: &[MachineName],
    version: Option<&str>,
) -> Vec<Result<(), String>> {
    let mut store = hub.store();
    let expired = match crate::assists::end_due(&mut store, Timestamp::now()) {
        Ok(expired) => expired,
        Err(error) => {
            return vec![
                Err(format!("the hub could not end an assistance past its deadline: {error}"));
                machines.len()
            ];
        }
    };
    let before = store.book.clone();
    let before_hub = store.hub.clone();
    let index = hub.index_of(&store);
    let directory = Indexed::new(&before.directory, &index);
    let actor = Actor {
        account: Some(account.clone()),
        authenticated: directory.logged_in(asker, account),
        member: Some(asker.clone()),
        peer: None,
    };
    let manifest = version.and_then(|version| hub.updates.lock().expect("never poisoned").manifest(version));
    let mut due: Vec<_> = store.hub.due.iter().cloned().map(Some).collect();
    let mut positions = BTreeMap::<_, Vec<usize>>::new();
    for (at, kept) in due.iter().enumerate() {
        positions.entry(kept.as_ref().expect("initial arrangement").key).or_default().push(at);
    }
    let mut succeeded = false;
    let mut acting_changed = false;
    let mut results = Vec::with_capacity(machines.len());
    let staged = (|| -> Result<(), String> {
        for machine in machines {
            let action = Action::UpdateDue { machine: machine.clone(), version: version.map(str::to_string) };
            let result = (|| {
                let found = directory.account(account).ok_or_else(|| format!("there is no account {account}"))?;
                if found.locked {
                    return Err(format!("the account {account} is locked"));
                }
                if !actor.authenticated {
                    return Err(format!("{asker} is not logged in to the account {account}"));
                }
                let member = directory.member(machine).ok_or_else(|| format!("there is no machine {machine}"))?;
                if member.home.is_none() {
                    return Err(format!("{machine} is not registered as a machine"));
                }
                if directory.authorize(account, Authority::Manage(machine)).is_err() {
                    return Err(format!("{machine} is not one of the account {account}'s machines"));
                }
                if let Some(version) = version
                    && fastctx_types::update::release(version).is_none()
                {
                    return Err(format!("{version} is not a version a release can have"));
                }
                if !succeeded {
                    for kept in &mut due {
                        if kept.as_ref().is_some_and(|kept| directory.by_key(&kept.key).is_none()) {
                            *kept = None;
                        }
                    }
                    store.book.directory.settle_acting();
                    acting_changed = store.book.directory.acting.len() != before.directory.acting.len();
                    crate::machine_log::effects(&mut store, &before.directory, &actor, &action)?;
                }
                if let Some(replaced) = positions.remove(&member.key) {
                    for at in replaced {
                        due[at] = None;
                    }
                }
                if let Some(version) = version {
                    let problem = manifest.is_none().then(|| Problem {
                        reason: FailureReason::Other,
                        why: format!("the manifest of {version} is not available yet"),
                    });
                    let arranged = Due {
                        version: version.to_string(),
                        by: account.clone(),
                        at: Timestamp::now(),
                        manifest: manifest.clone(),
                        problem,
                    };
                    positions.insert(member.key, vec![due.len()]);
                    due.push(Some(Arc::new(DueFor { key: member.key, due: arranged, batch: None })));
                }
                succeeded = true;
                Ok(())
            })();
            let outcome = match &result {
                Ok(()) => Outcome::Done,
                Err(reason) => Outcome::Refused { reason: reason.clone() },
            };
            store
                .book
                .machine_log
                .push(&before.directory, &before.directory.accounts, actor.clone(), action, outcome)
                .map_err(|error| format!("the hub could not keep the machine log: {error}"))?;
            results.push(result);
        }
        if succeeded {
            store.hub.due = due.into_iter().flatten().collect();
        }
        store.save_hub_and_book().map_err(|error| format!("the hub could not keep the update batch: {error}"))
    })();
    if let Err(reason) = staged {
        store.book = before;
        store.hub = before_hub;
        results = vec![Err(reason); machines.len()];
        succeeded = false;
    }
    drop(store);
    // Arrangements do not change directory visibility or afoot state. Only
    // actual assistance/acting changes need those network-wide projections.
    if expired || succeeded && acting_changed {
        hub.changed();
    }
    hub.machine_log_changed.notify_one();
    results
}
