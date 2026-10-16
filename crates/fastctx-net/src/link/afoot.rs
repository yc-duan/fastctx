//! What is afoot on this machine, told to the hub, and on the others, as the
//! hub tells it (`world-network.md`, 近况). Only what changed goes either
//! way: each session is first told all of it, then each change.
//!
//! What the hub told of the other machines stays when the line goes down,
//! to be shown as it last was; on the next session the hub tells all of it
//! again, machine by machine.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use fastctx_types::MachineName;
use fastctx_types::town::{Afoot, AfootChange, AfootSeen, ResidentNow, RunningJob};

use super::Inner;
use crate::wire::Up;

#[derive(Default)]
pub(crate) struct Afoots {
    here: Mutex<Here>,
    elsewhere: Mutex<BTreeMap<MachineName, Elsewhere>>,
    /// The session whose initial directory arrived.
    synced: Mutex<Option<u64>>,
    complete: Mutex<Option<u64>>,
}

/// What is afoot on this machine: the latest, and the session that takes it
/// with what it was told, none yet when it has been told nothing.
#[derive(Default)]
struct Here {
    latest: Option<Arc<Afoot>>,
    taking: Option<(u64, Option<Arc<Afoot>>)>,
    sending: Option<u64>,
    rejected: crate::afoot::isolation::Problems,
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
    pub(crate) fn tell_afoot(&self, mut afoot: Afoot) {
        crate::afoot::bounded::state(&mut afoot);
        let mut here = self.afoots.here.lock().expect("never poisoned");
        let rejected = crate::afoot::isolation::state(&mut afoot, here.latest.as_deref());
        crate::afoot::isolation::report(&mut here.rejected, rejected, |reason| self.node.log(reason));
        let afoot = Arc::new(afoot);
        if here.latest.as_ref() == Some(&afoot) {
            return;
        }
        here.latest = Some(afoot);
        self.send_afoot(&mut here);
    }

    /// The session's directory arrived, allowing local status to be sent.
    /// A newer hub acknowledges its initial status snapshot separately.
    pub(super) fn afoot_synced(&self, generation: u64) {
        if self.view().afoot {
            let mut here = self.afoots.here.lock().expect("never poisoned");
            here.taking = Some((generation, None));
            self.send_afoot(&mut here);
        }
        *self.afoots.synced.lock().expect("never poisoned") = Some(generation);
        self.node.afoot_changed();
    }

    pub(super) fn afoot_started(&self, generation: u64, pending: bool) {
        *self.afoots.complete.lock().expect("never poisoned") = (!pending).then_some(generation);
    }

    pub(super) fn afoot_complete(&self, generation: u64) {
        *self.afoots.complete.lock().expect("never poisoned") = Some(generation);
        self.node.afoot_changed();
    }

    /// Whether what the hub told of the other machines is as the session
    /// on now first gave it, or there is no session: false between a
    /// session's start and its first telling.
    pub(crate) fn afoot_ready(&self) -> bool {
        let Some(connection) = self.connection() else { return true };
        *self.afoots.synced.lock().expect("never poisoned") == Some(connection.generation)
            && *self.afoots.complete.lock().expect("never poisoned") == Some(connection.generation)
    }

    fn send_afoot(&self, here: &mut Here) {
        let Some((generation, _)) = &here.taking else { return };
        let generation = *generation;
        if here.latest.is_none() || here.sending == Some(generation) {
            return;
        }
        let Some(inner) = self.this.upgrade() else { return };
        here.sending = Some(generation);
        tokio::spawn(async move {
            if let Err(reason) = inner.write_afoot(generation).await {
                let mut here = inner.afoots.here.lock().expect("never poisoned");
                if here.sending == Some(generation) {
                    here.sending = None;
                }
                inner.node.log(format!("publishing status: {reason}"));
            }
        });
    }

    async fn write_afoot(&self, generation: u64) -> Result<(), String> {
        loop {
            let (before, after) = {
                let mut here = self.afoots.here.lock().expect("never poisoned");
                let Some((taking, before)) = &here.taking else { return Ok(()) };
                if *taking != generation {
                    return Ok(());
                }
                let Some(after) = &here.latest else {
                    here.sending = None;
                    return Ok(());
                };
                if before.as_ref() == Some(after) {
                    here.sending = None;
                    return Ok(());
                }
                (before.clone(), after.clone())
            };
            let connection = self
                .connection()
                .filter(|connection| connection.generation == generation)
                .ok_or_else(|| "the status connection ended".to_string())?;
            for frame in crate::afoot::Frames::new(before, after.clone()) {
                connection.up.write(Up::Afoot { change: Box::new(frame?) }).await?;
            }
            let mut here = self.afoots.here.lock().expect("never poisoned");
            if here.taking.as_ref().is_none_or(|(taking, _)| *taking != generation) {
                return Ok(());
            }
            here.taking = Some((generation, Some(after.clone())));
            if here.latest.as_ref() == Some(&after) {
                here.sending = None;
                return Ok(());
            }
        }
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
