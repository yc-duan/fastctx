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

mod publishing;
mod visibility;
pub(crate) use publishing::start;
use visibility::{seen, sees};

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use fastctx_net::directory::Directory;

use fastctx_types::town::{Afoot, AfootChange, AfootJob, AfootResident, AfootSeen};
use fastctx_types::{AccountName, MachineName, ResidentId};

use crate::Hub;
use fastctx_net::afoot::isolation::{self, Previous, Problems};

/// The latest each machine told, and what each member's session was told.
#[derive(Default)]
pub(crate) struct Afoots {
    told: HashMap<MachineName, Afoot>,
    prior: HashMap<MachineName, Previous>,
    shown: HashMap<MachineName, Shown>,
    rejected: HashMap<MachineName, Problems>,
    /// The directory as of the book's last keeping, copied once for each
    /// time it is kept or changed rather than for each change told: in a
    /// network of thousands, what is afoot changes many times a second.
    directory: Option<(u64, Arc<Directory>)>,
    publishing: Arc<publishing::Publishing>,
}

/// The directory as it is kept now, from the copy made since it was last
/// kept or changed.
fn directory(afoots: &mut Afoots, store: &crate::store::Store) -> Arc<Directory> {
    let saves = store.book_saves.load(std::sync::atomic::Ordering::SeqCst);
    match &afoots.directory {
        Some((kept, directory)) if *kept == saves => directory.clone(),
        _ => {
            let directory = Arc::new(store.book.directory.clone());
            afoots.directory = Some((saves, directory.clone()));
            directory
        }
    }
}

/// The directory changed: the next telling copies it again.
pub(crate) fn directory_changed(hub: &Hub) {
    hub.afoots().directory = None;
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
    Come,
}

/// `machine` told how what is afoot on it changed.
pub(crate) fn heard(hub: &Hub, machine: &MachineName, mut change: AfootChange<AfootResident, AfootJob>) {
    fastctx_net::afoot::bounded::change(&mut change);
    {
        let mut afoots = hub.afoots();
        let Afoots { told, prior, rejected, .. } = &mut *afoots;
        isolation::apply(
            told.entry(machine.clone()).or_default(),
            prior.entry(machine.clone()).or_default(),
            change,
            rejected.entry(machine.clone()).or_default(),
            |reason| hub.log.line(format_args!("from {machine}: {reason}")),
        );
    }
    hub.afoots().publishing.request(Some(machine));
}

/// Tells every member what changed in what it may see, after the directory
/// changed who sees what.
pub(crate) fn settle(hub: &Hub) {
    tell(hub, Scope::All);
}

/// Tells a session just come what is afoot on every machine it sees; the
/// others were told already.
pub(crate) fn came(hub: &Hub) {
    hub.afoots().publishing.request(None);
}

/// The accounts the conversation at `conversation` works for, as its
/// machine last told: `None` when it told nothing of it.
pub(crate) fn accounts_of(hub: &Hub, conversation: &fastctx_types::Address) -> Option<Vec<AccountName>> {
    let afoots = hub.afoots();
    let resident = afoots.told.get(&conversation.machine)?.residents.get(&conversation.resident)?;
    Some(resident.accounts.clone())
}

/// Tells each member reading what is afoot what changed in `scope`.
fn tell(hub: &Hub, scope: Scope<'_>) {
    // The sessions and the directory are read first, without the afoot
    // record's lock, which is taken last.
    let readers: Vec<(MachineName, u64, crate::outq::Outq)> = hub
        .sessions()
        .iter()
        .filter(|(_, session)| session.afoot)
        .map(|(name, session)| (name.clone(), session.generation, session.out.clone()))
        .collect();
    // Hold the directory until the state it will prune is locked. A newly
    // admitted machine cannot publish into a snapshot from before it joined.
    // Afoot writers never acquire the store while holding this leaf lock.
    let store = hub.store();
    let hidden = crate::forget::hidden(&store);
    let index = hub.index_of(&store);
    let mut afoots = hub.afoots();
    let directory = directory(&mut afoots, &store);
    let directory = fastctx_net::directory_index::Indexed::new(&directory, &index);
    drop(store);
    let Afoots { told, prior, shown, rejected, .. } = &mut *afoots;
    // A machine's last stays while it is on the hub under that name.
    told.retain(|machine, _| directory.member(machine).is_some_and(|member| member.name == *machine));
    let generations: HashMap<_, _> = readers.iter().map(|(name, generation, _)| (name, generation)).collect();
    shown.retain(|reader, shown| generations.get(reader).is_some_and(|generation| **generation == shown.generation));
    rejected.retain(|machine, _| told.contains_key(machine));
    prior.retain(|machine, _| told.contains_key(machine));
    let told = &*told;
    // What each set of logins sees of each machine, worked out once.
    let mut groups: HashMap<BTreeSet<AccountName>, usize> = HashMap::new();
    let mut seen_by: HashMap<(usize, &MachineName), Option<Arc<Seen>>> = HashMap::new();
    let mut candidates: HashMap<usize, Vec<&MachineName>> = HashMap::new();
    for (reader, generation, out) in readers {
        let shown = shown.entry(reader.clone()).or_insert_with(|| Shown {
            generation,
            fresh: true,
            machines: HashMap::new(),
            rejected: HashMap::new(),
        });
        if matches!(scope, Scope::Come) && !shown.fresh {
            continue;
        }
        let logins: BTreeSet<AccountName> =
            directory.member(&reader).map(|member| member.logins.clone()).unwrap_or_default();
        let next = groups.len();
        let group = *groups.entry(logins.clone()).or_insert(next);
        let machines: Vec<&MachineName> = match scope {
            Scope::Machines(machines) if !shown.fresh => {
                let candidates =
                    candidates.entry(group).or_insert_with(|| visibility::candidates(&directory, &logins, told));
                candidates
                    .iter()
                    .copied()
                    .filter(|machine| machines.contains(*machine))
                    .chain(
                        shown
                            .machines
                            .keys()
                            .filter(|machine| machines.contains(*machine))
                            .filter_map(|machine| told.get_key_value(machine).map(|(name, _)| name)),
                    )
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect()
            }
            _ => {
                let candidates =
                    candidates.entry(group).or_insert_with(|| visibility::candidates(&directory, &logins, told));
                // Previously visible machines must also be projected so revocation
                // sends an empty state rather than leaving stale private activity.
                let machines: HashSet<_> = candidates
                    .iter()
                    .copied()
                    .chain(
                        shown.machines.keys().filter_map(|machine| told.get_key_value(machine).map(|(name, _)| name)),
                    )
                    .collect();
                machines.into_iter().collect()
            }
        };
        for machine in machines {
            if *machine == reader {
                continue;
            }
            let mut now = seen_by
                .entry((group, machine))
                .or_insert_with(|| {
                    sees(&directory, &logins, machine)
                        .then(|| Arc::new(seen(&directory, &logins, machine, &told[machine], hidden.get(machine))))
                })
                .clone();
            let before = shown.machines.get(machine);
            if let Some(candidate) = &now {
                let mut after = candidate.afoot.clone();
                let rejected = isolation::snapshot(&mut after, before.map(|before| &*before.afoot));
                isolation::report(shown.rejected.entry(machine.clone()).or_default(), rejected, |reason| {
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
                    shown.machines.insert(machine.clone(), now);
                }
                None => {
                    shown.machines.remove(machine);
                }
            }
        }
        // Of machines that left, nothing more is told: they leave the
        // member's view, and it forgets them.
        shown.machines.retain(|machine, _| told.contains_key(machine));
        shown.rejected.retain(|machine, _| told.contains_key(machine));
        if shown.fresh {
            out.afoot_started();
            shown.fresh = false;
        }
    }
}
