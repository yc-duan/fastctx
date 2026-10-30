//! Version notices gather only the machines a recipient can manage. Readers
//! with the same scope share a snapshot, including while their lines are slow.
use super::inventory;
use crate::{Hub, outq::VersionSnapshot};
use fastctx_net::{
    authorization::Action,
    directory_index::{Indexed, ordered},
};
use fastctx_types::{AccountName, MachineName, update::Network};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Candidate positions are only an index optimization; the ordinary management
/// policy still decides every candidate, including locked and visitor accounts.
pub(super) fn managed(directory: &Indexed<'_>, account: &AccountName) -> Vec<usize> {
    let candidates = if directory.is_super(account) {
        (0..directory.all.members.len()).collect()
    } else {
        ordered(
            directory.owned(account).iter().copied().chain(
                directory
                    .shares_by_account(account)
                    .iter()
                    .filter_map(|&at| directory.position(&directory.all.shares[at].machine)),
            ),
        )
    };
    candidates
        .into_iter()
        .filter(|&at| directory.authorize(account, Action::Manage(&directory.all.members[at].name)).is_ok())
        .collect()
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Readers {
    accounts: Vec<AccountName>,
    own: Option<usize>,
    targets: Option<(Option<String>, Option<String>)>,
}

pub(super) fn tell(hub: &Hub, all: bool, initial: &BTreeSet<MachineName>) {
    let recipients: Vec<_> = hub
        .sessions()
        .iter()
        .filter(|(name, session)| session.versions && (all || initial.contains(*name)))
        .map(|(name, session)| (name.clone(), session.out.clone(), session.version_parts))
        .collect();
    if recipients.is_empty() {
        return;
    }
    let (directory, index, policy, due, repairs) = {
        let store = hub.store();
        (
            store.book.directory.clone(),
            hub.index_of(&store),
            store.hub.updates.clone(),
            store.hub.due.clone(),
            store.hub.repairs.clone(),
        )
    };
    let indexed = Indexed::new(&directory, &index);
    let network = super::network_in(hub, &indexed, policy, &due, &repairs);
    let offers = inventory::Offers::new(hub, &indexed);
    let due: BTreeMap<_, _> = due.into_iter().map(|kept| (kept.key, kept)).collect();
    let repairs: BTreeMap<_, _> = repairs.into_iter().map(|kept| (kept.key, kept)).collect();
    let mut aliases = BTreeMap::<usize, BTreeSet<MachineName>>::new();
    for name in network.machines.keys().chain(network.due.keys()).chain(network.repairs.keys()) {
        if let Some(at) = indexed.position(name) {
            aliases.entry(at).or_default().insert(name.clone());
        }
    }
    let mut accounts = BTreeMap::<AccountName, Arc<Vec<usize>>>::new();
    let mut combinations = BTreeMap::<Vec<AccountName>, Arc<Vec<usize>>>::new();
    let mut snapshots = BTreeMap::<Readers, Arc<Network>>::new();
    for (name, out, parts) in recipients {
        let me = indexed.member(&name);
        let logins: Vec<_> = me.into_iter().flat_map(|member| member.logins.iter().cloned()).collect();
        let common = combinations.entry(logins.clone()).or_insert_with(|| {
            let groups: Vec<_> = logins
                .iter()
                .map(|account| {
                    accounts.entry(account.clone()).or_insert_with(|| Arc::new(managed(&indexed, account))).clone()
                })
                .collect();
            if groups.len() == 1 {
                groups[0].clone()
            } else {
                Arc::new(ordered(groups.iter().flat_map(|group| group.iter().copied())))
            }
        });
        let own = indexed.position(&name).filter(|at| common.binary_search(at).is_err());
        let key = Readers { accounts: logins, own, targets: offers.scope(indexed.canonical(&name)) };
        let base = snapshots
            .entry(key)
            .or_insert_with(|| {
                let visible: BTreeSet<_> =
                    common.iter().copied().chain(own).filter_map(|at| aliases.get(&at)).flatten().collect();
                let mut cut = Network {
                    machines: visible
                        .iter()
                        .filter_map(|name| network.machines.get_key_value(*name).map(|(k, v)| (k.clone(), v.clone())))
                        .collect(),
                    due: visible
                        .iter()
                        .filter_map(|name| network.due.get_key_value(*name).map(|(k, v)| (k.clone(), v.clone())))
                        .collect(),
                    repairs: visible
                        .iter()
                        .filter_map(|name| network.repairs.get_key_value(*name).map(|(k, v)| (k.clone(), v.clone())))
                        .collect(),
                    stable: network.stable.clone(),
                    beta: network.beta.clone(),
                    policy: network.policy.clone(),
                    hub: network.hub.clone(),
                    updates_view: network.updates_view,
                    repair_supported: network.repair_supported,
                };
                offers.offer_to(indexed.canonical(&name), &mut cut);
                let previous = hub
                    .updates
                    .lock()
                    .expect("never poisoned")
                    .told
                    .get(&name)
                    .map(|snapshot| snapshot.network().clone());
                match previous {
                    Some(previous) if *previous == cut => previous,
                    _ => Arc::new(cut),
                }
            })
            .clone();
        // Only the executor gets the full manifests; the large visible map is shared.
        let update = me
            .and_then(|member| {
                due.get(&member.key)
                    .filter(|kept| base.due.get(&member.name).is_some_and(|ours| ours.version == kept.due.version))
            })
            .cloned();
        let repair = me
            .and_then(|member| {
                repairs
                    .get(&member.key)
                    .filter(|kept| base.repairs.get(&member.name).is_some_and(|ours| ours.id == kept.due.id))
            })
            .cloned();
        let snapshot = VersionSnapshot::new(base, me.map(|member| member.name.clone()), update, repair);
        let mut updates = hub.updates.lock().expect("never poisoned");
        if !initial.contains(&name) && updates.told.get(&name) == Some(&snapshot) {
            continue;
        }
        updates.told.insert(name, snapshot.clone());
        drop(updates);
        let _ = out.versions(snapshot, parts);
    }
}
