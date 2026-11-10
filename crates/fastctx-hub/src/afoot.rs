//! What is afoot on each machine (`world-network.md`, 近况): the latest each
//! machine told, kept in memory, and passed on to every member that reads
//! it as that member may see it, as changes. None of it is written down: it
//! is the moment, not a record. A machine that goes offline leaves its last
//! behind, shown as it was, until it tells anew or leaves the hub.
//!
//! A member sees what works for an account logged in on it, whatever the
//! machine, and nothing that works only for other accounts, the super
//! administrator included. The machine's owner is also told how many agents
//! are at work or waiting on it, whosever they are, save those of its
//! logins assisting other accounts there (`world-gui.md`, 机器页).
//!
//! Each session of a member is first told of every machine it sees that has
//! told anything, even of nothing there for it, so what it kept from an
//! earlier session is replaced; then of each change. What members logged in
//! to the same accounts see of a machine is the same, so it is worked out
//! once for them all and kept once.

#[cfg(feature = "change-drill")]
pub(crate) mod change_probe;
mod publishing;
mod state;
pub(crate) use state::Afoots;
mod visibility;
pub(crate) use publishing::start;
use visibility::{seen, sees};

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use fastctx_types::town::{Afoot, AfootChange, AfootJob, AfootResident, AfootSeen};
use fastctx_types::{AccountName, MachineName, ResidentId};

use crate::Hub;
use fastctx_net::afoot::isolation::{self, Previous, Problems};

/// Schedules a fresh projection after the committed directory changes.
pub(crate) fn directory_changed(hub: &Hub) {
    hub.afoots.publishing.all();
}

/// What one session of a member was told, machine by machine.
struct Shown {
    generation: u64,
    /// It has not been told of every machine yet.
    fresh: bool,
    machines: HashMap<MachineName, Arc<Seen>>,
    rejected: HashMap<MachineName, Problems>,
}

/// What a member was told of one machine.
#[derive(Default, PartialEq)]
struct Seen {
    afoot: Arc<AfootSeen>,
    busy: Option<u32>,
}

/// Whose telling is brought up to date, of which machines.
#[derive(Clone, Copy)]
enum Scope<'a> {
    /// Every session's, of every machine: who sees what may have changed.
    All,
    /// Every session's, of the machines that told something new.
    Machines(&'a HashSet<MachineName>),
    /// The sessions just come, of every machine.
    Come(&'a HashSet<MachineName>),
}

/// `machine` told how what is afoot on it changed.
pub(crate) fn heard(
    hub: &Hub,
    machine: &MachineName,
    key: fastctx_net::sign::PublicKey,
    mut change: AfootChange<AfootResident, AfootJob>,
) {
    fastctx_net::afoot::bounded::change(&mut change);
    born_keys(hub, machine, &mut change.residents);
    {
        let store = hub.store();
        if store.indexed().member(machine).is_none_or(|member| member.key != key) {
            return;
        }
        let member = hub.afoots.member(key, store.sequence);
        let mut status = member.lock();
        let state::Status { told, prior, rejected } = &mut *status;
        isolation::apply(Arc::make_mut(told), prior, change, rejected, |reason| {
            hub.log.line(format_args!("from {machine}: {reason}"))
        });
    }
    hub.afoots.publishing.request(machine);
}

/// Seals each conversation's birth as the hub knows it: the registered
/// birth, or for one not registered yet, the telling machine as it is now
/// when it was born there; none otherwise. The seal, not the machine's name,
/// says who holds it (`owned_records::born_on`). The account that opened it
/// goes first among those it works for: it is who others are told opened it.
fn born_keys(hub: &Hub, machine: &MachineName, residents: &mut [AfootResident]) {
    if residents.iter().all(|resident| resident.shared_on.is_none()) {
        return;
    }
    let store = hub.store();
    let directory = &store.book.directory;
    let owners = hub.account_records.owners.lock().expect("never poisoned");
    for resident in residents {
        resident.born_seal = None;
        let Some(born) = resident.shared_on.clone() else { continue };
        let address = fastctx_types::Address { machine: machine.clone(), resident: resident.resident.about.id.clone() };
        resident.born_seal = match owners.at(directory, &address) {
            Some(conversation) => {
                if let Some(at) = resident.accounts.iter().position(|account| *account == conversation.account) {
                    resident.accounts[..=at].rotate_right(1);
                }
                conversation
                    .shared_on
                    .as_ref()
                    .is_some_and(|shared| directory.same(shared, &born))
                    .then(|| conversation.born_seal.clone())
                    .flatten()
            }
            None if directory.same(&born, machine) => {
                directory.member(machine).and_then(fastctx_net::record_ownership::seal)
            }
            None => None,
        };
    }
}

/// Tells every member what changed in what it may see, after the directory
/// changed who sees what.
pub(crate) fn settle(hub: &Hub) {
    hub.afoots.publishing.all();
}

/// Tells a session just come what is afoot on every machine it sees; the
/// others were told already.
pub(crate) fn came(hub: &Hub, name: &MachineName) {
    hub.afoots.publishing.came(name);
}

/// The accounts the conversation at `conversation` works for, as its
/// machine last told: `None` when it told nothing of it.
pub(crate) fn owned_by(hub: &Hub, conversation: &fastctx_types::Address, account: &AccountName) -> Option<bool> {
    let store = hub.store();
    let key = store.indexed().member(&conversation.machine)?.key;
    let snapshot = hub.afoots.get(&key)?;
    let resident = snapshot.residents.get(&conversation.resident)?;
    let directory = &store.book.directory;
    Some(fastctx_net::record_ownership::allows(
        directory,
        account,
        &resident.accounts,
        resident
            .shared_on
            .as_ref()
            .filter(|born| crate::owned_records::born_there(directory, born, resident.born_seal.as_deref())),
    ))
}

/// Tells each member reading what is afoot what changed in `scope`.
fn tell(hub: &Hub, shown: &mut HashMap<MachineName, Shown>, scope: Scope<'_>) {
    let store = hub.store();
    let directory = store.indexed();
    let names: Vec<_> = match scope {
        Scope::All => {
            hub.afoots.retain(&directory, store.sequence);
            hub.sessions().keys().cloned().collect()
        }
        Scope::Come(names) => names.iter().cloned().collect(),
        Scope::Machines(machines) => machines
            .iter()
            .flat_map(|name| store.audience.readers(&directory, name))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect(),
    };
    let readers: Vec<_> = names
        .into_iter()
        .filter_map(|name| {
            hub.sessions()
                .get(&name)
                .filter(|session| {
                    session.afoot && directory.member(&name).is_some_and(|member| member.key == session.key)
                })
                .map(|session| (name.clone(), session.generation, session.out.clone(), session.replicated_records))
        })
        .collect();
    let hidden = crate::forget::hidden(&store);
    if matches!(scope, Scope::All) {
        shown
            .retain(|name, kept| hub.sessions().get(name).is_some_and(|session| session.generation == kept.generation));
    }
    let mut groups = HashMap::new();
    let mut projected: HashMap<(usize, MachineName), Option<Arc<Seen>>> = HashMap::new();
    let mut sources: HashMap<MachineName, Option<Arc<Afoot>>> = HashMap::new();
    for (reader, generation, out, replicated_records) in readers {
        let current = shown.entry(reader.clone()).or_insert_with(|| Shown {
            generation,
            fresh: true,
            machines: HashMap::new(),
            rejected: HashMap::new(),
        });
        if current.generation != generation {
            *current = Shown { generation, fresh: true, machines: HashMap::new(), rejected: HashMap::new() };
        }
        if matches!(scope, Scope::Come(_)) && !current.fresh {
            continue;
        }
        let logins = directory.member(&reader).map(|member| member.logins.clone()).unwrap_or_default();
        let next = groups.len();
        let group = *groups.entry((logins.clone(), replicated_records)).or_insert(next);
        let machines: HashSet<_> = match scope {
            Scope::Machines(machines) if !current.fresh => machines.iter().cloned().collect(),
            _ => visibility::candidates(&directory, &logins)
                .into_iter()
                .chain(current.machines.keys().cloned())
                .collect(),
        };
        for machine in machines {
            if machine == reader {
                continue;
            }
            let mut now = projected
                .entry((group, machine.clone()))
                .or_insert_with(|| {
                    if !sees(&directory, &logins, &machine) {
                        return None;
                    }
                    let source = sources
                        .entry(machine.clone())
                        .or_insert_with(|| directory.member(&machine).and_then(|member| hub.afoots.get(&member.key)));
                    source.as_ref().map(|source| {
                        Arc::new(seen(&directory, &logins, &machine, source, hidden.get(&machine), replicated_records))
                    })
                })
                .clone();
            let before = current.machines.get(&machine);
            if let Some(candidate) = &now {
                let mut after = candidate.afoot.clone();
                let rejected = isolation::snapshot(&mut after, before.map(|before| &*before.afoot));
                isolation::report(current.rejected.entry(machine.clone()).or_default(), rejected, |reason| {
                    hub.log.line(format_args!("from {machine} to {reader}: {reason}"))
                });
                if !Arc::ptr_eq(&after, &candidate.afoot) {
                    now = Some(Arc::new(Seen { afoot: after, busy: candidate.busy }));
                }
            }
            match (before, &now) {
                (None, None) => continue,
                (Some(was), Some(now)) if Arc::ptr_eq(was, now) || was == now => continue,
                _ => {}
            }
            let previous = before.map(|seen| seen.afoot.clone());
            let after = now.as_ref().map(|seen| seen.afoot.clone()).unwrap_or_default();
            let busy = now.as_ref().and_then(|seen| seen.busy);
            if out.afoot(machine.clone(), previous, after, busy).is_err() {
                continue;
            }
            match now {
                Some(now) => {
                    current.machines.insert(machine, now);
                }
                None => {
                    current.machines.remove(&machine);
                    current.rejected.remove(&machine);
                }
            }
        }
        if current.fresh {
            out.afoot_started();
            current.fresh = false;
        }
    }
}
