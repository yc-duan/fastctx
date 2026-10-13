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
//! earlier session is replaced; then of each change.

use std::collections::{BTreeSet, HashMap};

use fastctx_net::directory::Directory;
use fastctx_net::wire::Down;
use fastctx_types::town::{Afoot, AfootChange, AfootJob, AfootResident, AfootSeen};
use fastctx_types::{AccountName, MachineName, ResidentId};

use crate::{Hub, Out};

/// The latest each machine told, and what each member's session was told.
#[derive(Default)]
pub(crate) struct Afoots {
    told: HashMap<MachineName, Afoot>,
    shown: HashMap<MachineName, Shown>,
}

/// What one session of a member was told, machine by machine.
struct Shown {
    generation: u64,
    /// It has not been told of every machine yet.
    fresh: bool,
    machines: HashMap<MachineName, Seen>,
}

/// What a member was told of one machine.
#[derive(Default, PartialEq)]
struct Seen {
    afoot: AfootSeen,
    busy: Option<u32>,
}

/// `machine` told how what is afoot on it changed.
pub(crate) fn heard(hub: &Hub, machine: &MachineName, change: AfootChange<AfootResident, AfootJob>) {
    hub.afoots().told.entry(machine.clone()).or_default().apply(change);
    tell(hub, Some(machine));
}

/// Tells every member what changed in what it may see, after the directory
/// changed who sees what, or a machine or member came or went.
pub(crate) fn settle(hub: &Hub) {
    tell(hub, None);
}

/// The accounts the conversation at `conversation` works for, as its
/// machine last told: `None` when it told nothing of it.
pub(crate) fn accounts_of(hub: &Hub, conversation: &fastctx_types::Address) -> Option<Vec<AccountName>> {
    let afoots = hub.afoots();
    let resident = afoots.told.get(&conversation.machine)?.residents.get(&conversation.resident)?;
    Some(resident.accounts.clone())
}

/// Tells each member reading what is afoot what changed of `only`, or of
/// every machine.
fn tell(hub: &Hub, only: Option<&MachineName>) {
    // The sessions and the directory are read first, without the afoot
    // record's lock, which is taken last.
    let readers: Vec<(MachineName, u64, tokio::sync::mpsc::UnboundedSender<Out>)> = hub
        .sessions()
        .iter()
        .filter(|(_, session)| session.afoot)
        .map(|(name, session)| (name.clone(), session.generation, session.out.clone()))
        .collect();
    let (directory, hidden) = {
        let store = hub.store();
        (store.book.directory.clone(), crate::forget::hidden(&store))
    };
    let mut afoots = hub.afoots();
    let Afoots { told, shown } = &mut *afoots;
    // A machine's last stays while it is on the hub under that name.
    told.retain(|machine, _| directory.member(machine).is_some_and(|member| member.name == *machine));
    shown.retain(|reader, shown| {
        readers.iter().any(|(name, generation, _)| name == reader && *generation == shown.generation)
    });
    for (reader, generation, out) in readers {
        let shown =
            shown.entry(reader.clone()).or_insert_with(|| Shown { generation, fresh: true, machines: HashMap::new() });
        let logins: BTreeSet<AccountName> =
            directory.member(&reader).map(|member| member.logins.clone()).unwrap_or_default();
        let machines: Vec<&MachineName> = match only {
            Some(machine) if !shown.fresh => told.get_key_value(machine).map(|(name, _)| name).into_iter().collect(),
            _ => told.keys().collect(),
        };
        for machine in machines {
            if *machine == reader {
                continue;
            }
            let now = sees(&directory, &logins, machine)
                .then(|| seen(&directory, &logins, machine, &told[machine], hidden.get(machine)));
            let (change, busy) = match (shown.machines.get(machine), &now) {
                (None, None) => continue,
                (None, Some(now)) => (now.afoot.whole(), now.busy),
                (Some(_), None) => (AfootChange { anew: true, ..AfootChange::default() }, None),
                (Some(was), Some(now)) if was == now => continue,
                (Some(was), Some(now)) => (was.afoot.change_to(&now.afoot), now.busy),
            };
            let (frames, too_large) = fastctx_net::afoot::frames(change, fastctx_net::afoot::FRAME_ROOM);
            if !too_large.is_empty() {
                hub.log.line(format_args!(
                    "what is afoot on {machine} leaves out {} for {reader}: each is larger than a frame",
                    too_large.join(", ")
                ));
            }
            // A change of the count alone goes as a change of nothing.
            let frames = if frames.is_empty() { vec![AfootChange::default()] } else { frames };
            for change in frames {
                let _ = out.send(Out::Frame(Down::Afoot { machine: machine.clone(), busy, change: Box::new(change) }));
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
        shown.fresh = false;
    }
}

/// Whether a member logged in to `logins` sees `machine` at all, as its
/// view of the directory has it (`crate::views`).
fn sees(directory: &Directory, logins: &BTreeSet<AccountName>, machine: &MachineName) -> bool {
    let Some(member) = directory.member(machine) else { return false };
    logins.iter().any(|account| {
        directory.is_super(account) || member.logins.contains(account) || directory.rights(account, machine).any()
    })
}

/// What a member logged in to `logins` may see of what is afoot on `machine`;
/// of the conversations being deleted there (`hidden`), nothing.
fn seen(
    directory: &Directory,
    logins: &BTreeSet<AccountName>,
    machine: &MachineName,
    afoot: &Afoot,
    hidden: Option<&BTreeSet<ResidentId>>,
) -> Seen {
    let theirs = |accounts: &[AccountName]| accounts.iter().any(|account| logins.contains(account));
    let shown = |id: &ResidentId| hidden.is_none_or(|hidden| !hidden.contains(id));
    let owner =
        directory.member(machine).and_then(|member| member.home.as_ref()).is_some_and(|home| logins.contains(home));
    let mut seen = Seen::default();
    for (id, resident) in &afoot.residents {
        if theirs(&resident.accounts) && shown(id) {
            seen.afoot.residents.insert(id.clone(), resident.resident.clone());
        }
    }
    for (id, job) in &afoot.jobs {
        let of_hidden =
            job.job.owner.resident().is_some_and(|owner| owner.machine == *machine && !shown(&owner.resident));
        if theirs(&job.accounts) && !of_hidden {
            seen.afoot.jobs.insert(id.clone(), job.job.clone());
        }
    }
    // What works there for accounts that may work on the machine counts;
    // what its logins do assisting other accounts does not.
    seen.busy = owner.then(|| {
        let counted = afoot
            .residents
            .iter()
            .filter(|(id, _)| shown(id))
            .map(|(_, resident)| resident)
            .filter(|resident| resident.resident.in_progress())
            .filter(|resident| resident.accounts.iter().any(|account| directory.rights(account, machine).hands))
            .count();
        u32::try_from(counted).unwrap_or(u32::MAX)
    });
    seen
}
