//! What is afoot on this machine, told to the hub, and on the others, as the
//! hub tells it (`world-network.md`, 近况). Only what changed goes either
//! way: each session is first told all of it, then each change.
//!
//! What the hub told of the other machines stays when the line goes down,
//! to be shown as it last was; on the next session the hub tells all of it
//! again, machine by machine.

use std::collections::BTreeMap;
use std::sync::Mutex;

use fastctx_types::MachineName;
use fastctx_types::town::{Afoot, AfootChange, AfootSeen, ResidentNow, RunningJob};

use super::Inner;
use crate::wire::Up;

#[derive(Default)]
pub(crate) struct Afoots {
    here: Mutex<Here>,
    elsewhere: Mutex<BTreeMap<MachineName, Elsewhere>>,
    /// The session whose first telling of the other machines has come.
    synced: Mutex<Option<u64>>,
}

/// What is afoot on this machine: the latest, and the session that takes it
/// with what it was told, none yet when it has been told nothing.
#[derive(Default)]
struct Here {
    latest: Option<Afoot>,
    taking: Option<(u64, Option<Afoot>)>,
}

/// What is afoot on another machine as the hub told this one: what there
/// works for an account logged in here, and, when this computer owns that
/// machine, how many agents are at work or waiting there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Elsewhere {
    pub seen: AfootSeen,
    pub busy: Option<u32>,
}

impl Inner {
    /// What is afoot on this machine is `afoot` now: told to the hub as the
    /// session there takes it.
    pub(crate) fn tell_afoot(&self, afoot: Afoot) {
        let mut here = self.afoots.here.lock().expect("never poisoned");
        if here.latest.as_ref() == Some(&afoot) {
            return;
        }
        here.latest = Some(afoot);
        self.send_afoot(&mut here);
    }

    /// The session `generation` has asked the hub for the view: its first
    /// telling of the other machines came before the answer. When the hub
    /// takes what is afoot, it is told all of it here.
    pub(super) fn afoot_synced(&self, generation: u64) {
        if self.view().afoot {
            let mut here = self.afoots.here.lock().expect("never poisoned");
            here.taking = Some((generation, None));
            self.send_afoot(&mut here);
        }
        *self.afoots.synced.lock().expect("never poisoned") = Some(generation);
        self.node.afoot_changed();
    }

    /// Whether what the hub told of the other machines is as the session
    /// on now first gave it, or there is no session: false between a
    /// session's start and its first telling.
    pub(crate) fn afoot_ready(&self) -> bool {
        let Some(connection) = self.connection() else { return true };
        *self.afoots.synced.lock().expect("never poisoned") == Some(connection.generation)
    }

    fn send_afoot(&self, here: &mut Here) {
        let Some(latest) = &here.latest else { return };
        let Some((generation, told)) = &mut here.taking else { return };
        let Some(connection) = self.connection().filter(|connection| connection.generation == *generation) else {
            return;
        };
        let change = match told {
            Some(told) => told.change_to(latest),
            None => latest.whole(),
        };
        if change.is_empty() {
            return;
        }
        let (frames, too_large) = crate::afoot::frames(change, crate::afoot::FRAME_ROOM);
        if !too_large.is_empty() {
            self.node.log(format!(
                "what is afoot on this machine leaves out {}: each is larger than a frame to the hub",
                too_large.join(", ")
            ));
        }
        for change in frames {
            if connection.up.send(Up::Afoot { change: Box::new(change) }).is_err() {
                return;
            }
        }
        *told = Some(latest.clone());
    }

    /// The hub told how what is afoot on `machine` changed.
    pub(super) fn heard_afoot(
        &self,
        machine: MachineName,
        busy: Option<u32>,
        change: AfootChange<ResidentNow, RunningJob>,
    ) {
        {
            let mut elsewhere = self.afoots.elsewhere.lock().expect("never poisoned");
            let afoot = elsewhere.entry(machine).or_default();
            afoot.seen.apply(change);
            afoot.busy = busy;
        }
        self.node.afoot_changed();
    }

    /// Forgets what was told of machines this machine's view no longer has.
    pub(super) fn prune_afoot(&self) {
        let view = self.view();
        let pruned = {
            let mut elsewhere = self.afoots.elsewhere.lock().expect("never poisoned");
            let before = elsewhere.len();
            elsewhere.retain(|machine, _| view.machines().any(|member| member.name == *machine));
            elsewhere.len() != before
        };
        if pruned {
            self.node.afoot_changed();
        }
    }

    pub(crate) fn afoot_elsewhere(&self) -> BTreeMap<MachineName, Elsewhere> {
        self.afoots.elsewhere.lock().expect("never poisoned").clone()
    }
}
