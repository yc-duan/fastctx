//! Distinguish changes to record inputs from the directory's displayed state.

use std::sync::Arc;

use crate::directory::{Presence, View};

impl View {
    /// Whether authorization or replicated-record consumers need to refresh.
    /// Presence, grouping, notices and revision counters have separate consumers.
    /// Every other field remains significant, including fields added in the future.
    pub fn records_changed(&self, previous: &View) -> bool {
        self.changed(previous, false)
    }

    /// Whether connectivity or authority changed. Group labels and notices
    /// still reach display consumers without waking callers waiting for a route.
    pub fn ways_changed(&self, previous: &View) -> bool {
        self.changed(previous, true)
    }

    fn changed(&self, previous: &View, presence: bool) -> bool {
        let mut next = self.clone();
        next.revision = previous.revision;
        next.presence = previous.presence;
        next.machine_notices.clone_from(&previous.machine_notices);
        for (now, was) in next.directory.members.iter_mut().zip(&previous.directory.members) {
            if now.name == was.name && now != was {
                let now = Arc::make_mut(now);
                if !presence {
                    Presence::of(was).apply(now);
                }
                now.group.clone_from(&was.group);
            }
        }
        next != *previous
    }
}
