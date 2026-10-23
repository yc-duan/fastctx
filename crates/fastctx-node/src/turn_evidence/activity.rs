//! Last actual progress is in memory; streaming does not add disk writes.

use fastctx_types::ResidentId;
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use jiff::Timestamp;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone)]
pub(super) struct Activity {
    pub turn: String,
    pub began: Timestamp,
    pub last: Timestamp,
}

#[derive(Default)]
pub(crate) struct Activities(Mutex<HashMap<ResidentId, Activity>>);

impl Activities {
    pub(super) fn get(&self, resident: &ResidentId) -> Option<Activity> {
        self.0.lock().expect("never poisoned").get(resident).cloned()
    }

    pub(super) fn observe(&self, resident: &ResidentId, event: &SessionEvent) {
        let mut rows = self.0.lock().expect("never poisoned");
        if let Happened::TurnStarted { turn } = &event.happened {
            match rows.get_mut(resident) {
                Some(row) if row.turn == *turn => row.last = row.last.max(event.at),
                _ => {
                    rows.insert(resident.clone(), Activity { turn: turn.clone(), began: event.at, last: event.at });
                }
            }
            return;
        }
        if let Some(turn) = progress(&event.happened)
            && let Some(row) = rows.get_mut(resident)
            && turn.is_none_or(|turn| row.turn == turn)
        {
            row.last = row.last.max(event.at);
        }
    }

    pub(super) fn forget(&self, resident: &ResidentId, turn: &str) {
        let mut rows = self.0.lock().expect("never poisoned");
        if rows.get(resident).is_some_and(|row| row.turn == turn) {
            rows.remove(resident);
        }
    }
}

fn progress(event: &Happened) -> Option<Option<&str>> {
    match event {
        Happened::Began { turn, .. } | Happened::Updated { turn, .. } | Happened::Finished { turn, .. } => {
            Some(Some(turn))
        }
        Happened::Grew { turn, text, .. } if !text.is_empty() => Some(Some(turn)),
        Happened::Changed { turn, change: SessionChange::AgentEvent { happened, .. } }
            if progress(happened).is_some() =>
        {
            Some(turn.as_deref())
        }
        Happened::Changed { turn, change: SessionChange::AgentStarted { .. } | SessionChange::AgentEnded { .. } } => {
            Some(turn.as_deref())
        }
        // Status and retry notifications do not establish progress.
        _ => None,
    }
}
