//! The current hub host is never shared. Identification and revocation use
//! the same store lock as sharing and moving, so those operations cannot race.

use crate::{Hub, store::Store};
use fastctx_types::{
    MachineName,
    machine_log::{Action, Actor, Outcome},
};

pub(crate) fn current(hub: &Hub, store: &Store) -> Option<MachineName> {
    let directory = &store.book.directory;
    // A completed move supersedes reports kept from the old hub host.
    if let Some(incoming) = store.moves.last() {
        let index = hub.index_of(store);
        let indexed = fastctx_net::directory_index::Indexed::new(directory, &index);
        if let Some(member) = indexed.member(&incoming.to) {
            return Some(member.name.clone());
        }
    }
    let hosts = crate::updates::hub_hosts(hub);
    if hosts.is_empty() {
        return None;
    }
    let index = hub.index_of(store);
    let indexed = fastctx_net::directory_index::Indexed::new(directory, &index);
    hosts
        .iter()
        .filter_map(|key| indexed.key_position(key))
        .filter(|&at| directory.members[at].is_machine())
        .min()
        .map(|at| directory.members[at].name.clone())
}

pub(crate) fn protected(hub: &Hub, store: &Store, machine: &MachineName) -> bool {
    let directory = &store.book.directory;
    current(hub, store).is_some_and(|host| directory.same(&host, machine))
        || store.outgoing.as_ref().is_some_and(|moving| moving.pending() && directory.same(&moving.to, machine))
}

/// Removes legacy shares in the same commit as their change records. The
/// ordinary directory push delivers the revocation to every affected device.
pub(crate) fn settle(hub: &Hub) -> Result<(), String> {
    let mut store = hub.store();
    let Some(machine) = current(hub, &store) else { return Ok(()) };
    let shares: Vec<_> = store
        .book
        .directory
        .shares
        .iter()
        .filter(|share| store.book.directory.same(&share.machine, &machine))
        .cloned()
        .collect();
    if shares.is_empty() {
        return Ok(());
    }
    let before = store.book.clone();
    store.book.directory.shares.retain(|share| !before.directory.same(&share.machine, &machine));
    store.book.directory.settle_acting();
    let actor = Actor { account: None, authenticated: true, member: None, peer: None };
    let result = (|| {
        for (index, share) in shares.into_iter().enumerate() {
            let action =
                Action::Share { machine: machine.clone(), to: share.account.clone(), shared: false, jobs: None };
            if index == 0 {
                crate::machine_log::effects(&mut store, &before.directory, &actor, &action)?;
            }
            let accounts = store.book.directory.accounts.clone();
            store
                .book
                .machine_log
                .push(&before.directory, &accounts, actor.clone(), action, Outcome::Done)
                .map_err(|error| error.to_string())?;
        }
        store.save_book().map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        store.book = before;
        return Err(error);
    }
    drop(store);
    hub.changed();
    Ok(())
}
