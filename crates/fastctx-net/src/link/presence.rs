//! Who is online, as the hub tells it changes (`crate::wire::Down::Presence`)
//! between the views it gives: each telling is how one machine is
//! connected now, numbered among the hub's changes of who is online. A view
//! counts the changes it holds (`View::presence`); a telling no newer than
//! that is in it already, and one newer is put in it, also once a view
//! comes after the telling did.

use std::sync::Arc;

use fastctx_types::MachineName;

use super::Inner;
use crate::directory::{Presence, View};

impl Inner {
    /// The hub told how `machine` is connected now, its `rev`th change of
    /// who is online.
    pub(super) fn heard_presence(&self, machine: MachineName, mut presence: Presence, rev: u64) {
        presence.since = presence.since.map(|at| self.local(at));
        {
            let mut presences = self.presences.lock().expect("never poisoned");
            if presences.get(&machine).is_some_and(|(told, _)| *told >= rev) {
                return;
            }
            presences.insert(machine.clone(), (rev, presence.clone()));
        }
        let changed = {
            let mut view = self.view.lock().expect("never poisoned");
            let now =
                view.directory.members.iter().find(|member| member.name == machine).map(|member| Presence::of(member));
            if rev <= view.presence || now.is_none_or(|now| now == presence) {
                false
            } else {
                let mut next = View::clone(&view);
                if let Some(member) = next.directory.members.iter_mut().find(|member| member.name == machine) {
                    presence.apply(Arc::make_mut(member));
                }
                *view = Arc::new(next);
                true
            }
        };
        if changed {
            self.ways_changed();
            self.node.presence_changed();
        }
    }

    /// Puts in `view`, newly given, what was told of who is online after
    /// it was made; forgets what it holds already.
    pub(super) fn with_presences(&self, view: &mut View) {
        let mut presences = self.presences.lock().expect("never poisoned");
        presences.retain(|_, (rev, _)| *rev > view.presence);
        for (machine, (_, presence)) in presences.iter() {
            if let Some(member) = view.directory.members.iter_mut().find(|member| member.name == *machine) {
                presence.apply(Arc::make_mut(member));
            }
        }
    }
}
