//! A Claude resident's history as Claude Code keeps it (`world-colleagues.md`,
//! 用户在任何地方和居民说话): each of its sessions read from Claude Code's
//! own record of it, the one host file the World reads (`world.md`), and
//! placed in the turns the World saw. The World's own record says who spoke
//! and how each turn ended; it keeps no copy of the conversation.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use fastctx_hosts::claude::{self, exchange, transcript};
use fastctx_types::session::{Happened, Health, HealthKind, SessionEvent};
use fastctx_types::{ProcessIdentity, ResidentId};
use jiff::Timestamp;

use crate::Node;

/// What Claude Code kept of the resident's sessions, oldest first, placed
/// in the World's turns of `recorded` where the World saw them happen.
pub async fn hosted(node: &Arc<Node>, resident: &ResidentId, recorded: &[SessionEvent]) -> Vec<SessionEvent> {
    let root = exchange::sessions(node.world.path());
    let (sessions, problems) = sessions(node, resident);
    let mut events: Vec<_> = problems.into_iter().map(|problem| note(Timestamp::now(), problem)).collect();
    for Session { id, began, process, kept } in sessions {
        match read_whole(root.join(&id), kept, process).await {
            Ok(read) => {
                if let Some(title) = read.title {
                    events.push(SessionEvent { at: Timestamp::now(), happened: Happened::Titled { title } });
                }
                events.extend(read.events);
                if !read.problems.is_empty() {
                    let text = format!(
                        "{} part(s) of Claude Code's record of session {id} could not be read: {}",
                        read.problems.len(),
                        read.problems.join("; ")
                    );
                    events.push(note(began, text));
                }
            }
            Err(why) => events.push(note(began, format!("Claude Code's record of session {id}: {why}"))),
        }
    }
    place(events, recorded)
}

/// CLI forking reads the saved conversation. Wait for the last message
/// already observed by the mod before asking the CLI to copy that file.
pub(super) async fn wait_stored(node: &Arc<Node>, session: &str) -> Result<(), String> {
    let (kept, process) = {
        let state = node.claude.state.lock().expect("never poisoned");
        (
            state.stored.get::<String>(session),
            exchange::registration(&exchange::sessions(node.world.path()).join(session))
                .map_err(|error| error.to_string())?
                .and_then(|registration| registration.process),
        )
    };
    if kept.is_none() {
        return Ok(());
    }
    let read = read_whole(exchange::sessions(node.world.path()).join(session), kept, process).await?;
    if read.problems.is_empty() { Ok(()) } else { Err(read.problems.join("; ")) }
}

/// A session the resident has been, as its mod registered it.
struct Session {
    id: String,
    began: Timestamp,
    process: Option<ProcessIdentity>,
    /// The last record its mod saw Claude Code keep, in this run of the node.
    kept: Option<String>,
}

/// The sessions the resident has been, the oldest first.
fn sessions(node: &Node, resident: &ResidentId) -> (Vec<Session>, Vec<String>) {
    let state = node.claude.state.lock().expect("never poisoned");
    let mut sessions = Vec::new();
    let mut problems = Vec::new();
    let scanned = (|| -> std::io::Result<()> {
        for entry in std::fs::read_dir(exchange::sessions(node.world.path()))? {
            let directory = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    problems.push(format!("reading a Claude session directory: {error}"));
                    continue;
                }
            };
            let read = (|| -> std::io::Result<Option<Session>> {
                let alias = match std::fs::read(directory.join("resident.json")) {
                    Ok(bytes) => serde_json::from_slice::<ResidentId>(&bytes).map_err(std::io::Error::other)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => return Err(error),
                };
                if &alias != resident {
                    return Ok(None);
                }
                if let Some(registration) = exchange::registration(&directory)? {
                    return Ok(Some(Session {
                        kept: state.stored.get(&registration.session),
                        id: registration.session,
                        began: registration.at.unwrap_or_default(),
                        process: registration.process,
                    }));
                }
                Ok(None)
            })();
            match read {
                Ok(Some(session)) => sessions.push(session),
                Ok(None) => {}
                Err(error) => problems.push(format!("reading {}: {error}", directory.display())),
            }
        }
        Ok(())
    })();
    if let Err(error) = scanned {
        problems.push(format!("reading {resident}'s Claude sessions: {error}"));
    }
    sessions.sort_by_key(|session| session.began);
    (sessions, problems)
}

/// Reads a session's record once it holds `kept`. Claude Code writes its
/// file a while after it keeps a record, with no bound on how long and no
/// sign when it has (2.1.289): what was kept just before is waited for,
/// read again at each change to the file, until it is there or the
/// session's process has ended.
async fn read_whole(
    directory: PathBuf,
    kept: Option<String>,
    process: Option<ProcessIdentity>,
) -> Result<transcript::Transcript, String> {
    let cuts = super::rewind::cuts(&directory).map_err(|error| error.to_string())?;
    let path = exchange::transcript(&directory)
        .map_err(|error| format!("where it is kept could not be read: {error}"))?
        .ok_or("the World's plugin never said where Claude Code keeps it")?;
    let Some(kept) = kept else { return read_now(path, cuts).await };
    // Watched before it is read, so no change between the two goes unseen.
    let (changed, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |_: notify::Result<notify::Event>| {
        let _ = changed.send(());
    })
    .map_err(|error| format!("watching {}: {error}", path.display()))?;
    let folder = path.parent().ok_or_else(|| format!("{} has no folder", path.display()))?;
    notify::Watcher::watch(&mut watcher, folder, notify::RecursiveMode::NonRecursive)
        .map_err(|error| format!("watching {}: {error}", folder.display()))?;
    let mut ended: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>> = match process {
        Some(process) => Box::pin(async move { fastctx_platform::host_process::exited(&process).await }),
        None => Box::pin(std::future::pending()),
    };
    loop {
        let read = read_now(path.clone(), cuts.clone()).await?;
        if read.uuids.contains(&kept) {
            return Ok(read);
        }
        tokio::select! {
            Some(()) = changes.recv() => {}
            result = &mut ended => {
                result.map_err(|error| format!("observing Claude's process: {error}"))?;
                let mut read = read_now(path.clone(), cuts.clone()).await?;
                if !read.uuids.contains(&kept) {
                    read.problems.push("its process ended before its file held the last of what it kept".to_string());
                }
                return Ok(read);
            }
        }
    }
}

/// The record as the file holds it now. A session at work adds to its file
/// as it goes; a read it overtook is read again, until one sees it whole.
async fn read_now(path: PathBuf, cuts: Vec<transcript::HistoryCut>) -> Result<transcript::Transcript, String> {
    loop {
        let reading = path.clone();
        let cuts = cuts.clone();
        let read = tokio::task::spawn_blocking(move || transcript::read_with_cuts(&reading, &cuts))
            .await
            .map_err(|error| format!("reading it: {error}"))?;
        match read {
            Ok(read) => return Ok(read),
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
pub(super) fn place(mut events: Vec<SessionEvent>, recorded: &[SessionEvent]) -> Vec<SessionEvent> {
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
                Happened::Heard { turn: slot, .. }
                | Happened::Health { turn: slot, .. }
                | Happened::Changed { turn: slot, .. } => *slot = Some(turn),
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
            if told && fastctx_types::session::attachments::AttachedMessage::read(text).is_none() {
                continue;
            }
        }
        placed.push(event);
    }
    placed
}
