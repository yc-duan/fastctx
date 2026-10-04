//! A Claude resident's history as Claude Code keeps it (`world-colleagues.md`,
//! 用户在任何地方和居民说话): each of its sessions read from Claude Code's
//! own record of it, the one host file the World reads (`world.md`), and
//! placed in the turns the World saw. The World's own record says who spoke
//! and how each turn ended; it keeps no copy of the conversation.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use fastctx_hosts::claude::{self, exchange, transcript};
use fastctx_types::ResidentId;
use fastctx_types::session::{Happened, Health, HealthKind, SessionEvent};
use jiff::Timestamp;

use crate::Node;

/// What Claude Code kept of the resident's sessions, oldest first, placed
/// in the World's turns of `recorded` where the World saw them happen.
pub async fn hosted(node: &Arc<Node>, resident: &ResidentId, recorded: &[SessionEvent]) -> Vec<SessionEvent> {
    let root = exchange::sessions(node.world.path());
    let mut events = Vec::new();
    for (session, began) in sessions(node, resident) {
        let directory = root.join(&session);
        let read = tokio::task::spawn_blocking(move || read(&directory)).await;
        match read {
            Ok(Ok(read)) => {
                events.extend(read.events);
                if !read.problems.is_empty() {
                    let text = format!(
                        "{} part(s) of Claude Code's record of session {session} could not be read: {}",
                        read.problems.len(),
                        read.problems.join("; ")
                    );
                    events.push(note(began, text));
                }
            }
            Ok(Err(why)) => events.push(note(began, format!("Claude Code's record of session {session}: {why}"))),
            Err(error) => {
                events.push(note(began, format!("reading Claude Code's record of session {session}: {error}")))
            }
        }
    }
    place(events, recorded)
}

/// The sessions the resident has been, as their mod registered them, the
/// oldest first, with when each began.
fn sessions(node: &Node, resident: &ResidentId) -> Vec<(String, Timestamp)> {
    let state = node.claude.state.lock().expect("never poisoned");
    let mut sessions: Vec<(String, Timestamp)> = state
        .sessions
        .iter()
        .filter(|(_, session)| session.resident.as_ref() == Some(resident))
        .map(|(id, session)| (id.clone(), session.registration.at.unwrap_or_default()))
        .collect();
    sessions.sort_by_key(|(_, at)| *at);
    sessions
}

fn read(directory: &Path) -> Result<transcript::Transcript, String> {
    let path = exchange::transcript(directory)
        .map_err(|error| format!("where it is kept could not be read: {error}"))?
        .ok_or("the World's plugin never said where Claude Code keeps it")?;
    loop {
        match transcript::read(&path) {
            Ok(read) => return Ok(read),
            // A session at work adds to its file as it goes; a read it
            // overtook is read again, until one sees the file whole.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("{} is gone; Claude Code removes old sessions itself", path.display()));
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
}

/// An item the host deliberately keeps out of its conversation.
fn hidden(event: &SessionEvent) -> bool {
    matches!(&event.happened, Happened::Finished { item, .. } if matches!(item.what, fastctx_types::session::What::Hidden { .. }))
}

fn note(at: Timestamp, text: String) -> SessionEvent {
    SessionEvent { at, happened: Happened::Health { turn: None, health: Health { kind: HealthKind::Warning, text } } }
}

/// Puts each event of Claude Code's record in the World's turn it happened
/// in: the turn running at its time, and for words heard just before a turn
/// started with nothing else between, that turn, as they are what started
/// it. What the World did not see stays where it is, in no turn. Words the
/// World itself handed over are told by the World's record, which knows
/// who said them, and are left out here.
fn place(mut events: Vec<SessionEvent>, recorded: &[SessionEvent]) -> Vec<SessionEvent> {
    let mut turns: Vec<(Timestamp, Option<Timestamp>, String)> = Vec::new();
    let mut ends: HashMap<&str, Timestamp> = HashMap::new();
    let mut heard: HashMap<&str, Vec<&str>> = HashMap::new();
    for event in recorded {
        match &event.happened {
            Happened::TurnEnded { turn, .. } => {
                ends.insert(turn, event.at);
            }
            Happened::Heard { turn: Some(turn), text, .. } => heard.entry(turn).or_default().push(text),
            _ => {}
        }
    }
    for event in recorded {
        if let Happened::TurnStarted { turn } = &event.happened {
            turns.push((event.at, ends.get(turn.as_str()).copied(), turn.clone()));
        }
    }
    turns.sort_by_key(|(start, ..)| *start);
    events.sort_by_key(|event| event.at);
    let within = |at: Timestamp| {
        turns
            .iter()
            .rev()
            .find(|(start, end, _)| *start <= at && end.is_none_or(|end| at <= end))
            .map(|(.., turn)| turn)
    };
    let mut placed = Vec::with_capacity(events.len());
    for (index, mut event) in events.iter().cloned().enumerate() {
        let turn = within(event.at).cloned().or_else(|| match event.happened {
            Happened::Heard { .. } => {
                let (start, _, turn) = turns.iter().find(|(start, ..)| *start >= event.at)?;
                // What the host keeps out of its conversation, such as the
                // context it gives the model with the words, is not between.
                let next = events[index + 1..].iter().find(|next| !hidden(next)).map(|next| next.at);
                next.is_none_or(|next| next >= *start).then(|| turn.clone())
            }
            _ => None,
        });
        if let Some(turn) = turn {
            match &mut event.happened {
                Happened::Heard { turn: slot, .. } | Happened::Health { turn: slot, .. } => *slot = Some(turn),
                Happened::Began { turn: slot, .. }
                | Happened::Grew { turn: slot, .. }
                | Happened::Finished { turn: slot, .. }
                | Happened::Steps { turn: slot, .. } => *slot = turn,
                _ => {}
            }
        }
        if let Happened::Heard { turn, text, .. } = &event.happened {
            let told = claude::is_framed(text)
                || turn.as_deref().and_then(|turn| heard.get(turn)).is_some_and(|said| said.contains(&text.as_str()));
            if told {
                continue;
            }
        }
        placed.push(event);
    }
    placed
}
