//! A newly opened view receives only requests that remain answerable.
//! Its subscription starts before the host history and pending snapshot.

use std::collections::HashSet;
use std::sync::Arc;

use fastctx_hosts::codex::CodexHost;
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use fastctx_net::watch_queue::{self, Events};

pub fn recorded(recorded: Vec<SessionEvent>, hosted: &[SessionEvent]) -> Vec<SessionEvent> {
    let turns: HashSet<_> = hosted.iter().filter_map(|event| crate::talk::turn_of(&event.happened)).collect();
    // Absence from host storage is not itself a rewind: a failed startup or
    // an in-flight turn can exist only in the World's record. Retire absent
    // turns only across an explicit host history-replacement checkpoint.
    let checkpoint = recorded
        .iter()
        .rposition(|event| matches!(event.happened, Happened::Changed { change: SessionChange::HistoryChanged, .. }));
    let discarded: HashSet<String> = checkpoint
        .into_iter()
        .flat_map(|index| recorded[..index].iter())
        .filter_map(|event| crate::talk::turn_of(&event.happened))
        .filter(|turn| !turns.contains(turn))
        .map(str::to_string)
        .collect();
    recorded
        .into_iter()
        .filter(|event| {
            !matches!(
                &event.happened,
                Happened::Changed { change: SessionChange::Request { .. } | SessionChange::Resolved { .. }, .. }
            ) && crate::talk::turn_of(&event.happened).is_none_or(|turn| !discarded.contains(turn))
        })
        .collect()
}

pub fn requests(
    host: Arc<CodexHost>,
    thread: String,
    mut events: Events,
) -> Events {
    let (sender, receiver) = watch_queue::events();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                event = events.recv() => {
                    let event = match event {
                        Ok(Some(event)) => event,
                        Ok(None) => return,
                        Err(_) => { sender.lag(); return; },
                    };
                    if let Happened::Changed { change: SessionChange::Request { id, .. }, .. } = &event.happened
                        && !host.request_pending(&thread, id) { continue }
                    if sender.send(event).is_err() { return }
                }
                () = sender.closed() => return,
            }
        }
    });
    receiver
}
