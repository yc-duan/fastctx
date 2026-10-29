//! The mod changes context; this adapter observes the host's persisted replacement.

use std::sync::Arc;

use fastctx_hosts::claude::{control as files, exchange, transcript};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use jiff::Timestamp;
use serde_json::{Value, json};

use crate::Node;

pub(super) async fn finish(
    node: &Arc<Node>,
    resident: &ResidentId,
    reply: &Value,
    from: Option<&str>,
) -> Result<(), Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let session = about.session.ok_or_else(|| Failure::new("Claude has not identified the fork yet"))?;
    let directory = exchange::sessions(node.world.path()).join(&session);
    let cut = transcript::HistoryCut {
        until: reply["retained_until"]
            .as_str()
            .ok_or_else(|| Failure::new("Claude rewind omitted its retained anchor"))?
            .into(),
        parent: reply["previous_tail"]
            .as_str()
            .ok_or_else(|| Failure::new("Claude rewind omitted its prior context tail"))?
            .into(),
        messages: reply["retained_messages"]
            .as_u64()
            .ok_or_else(|| Failure::new("Claude rewind omitted its message count"))?
            .try_into()
            .map_err(|_| Failure::new("Claude rewind message count exceeds this platform"))?,
        retained: serde_json::from_value(reply["retained"].clone())
            .map_err(|error| Failure::new(format!("Claude rewind retained projections: {error}")))?,
    };
    let path = directory.join("history-cuts.json");
    let mut cuts = files::read::<Vec<transcript::HistoryCut>>(&path).map_err(failure)?.unwrap_or_default();
    cuts.push(cut);
    files::write(&path, &cuts).map_err(failure)?;
    let transcript = exchange::transcript(&directory)
        .map_err(failure)?
        .ok_or_else(|| Failure::new("Claude has not identified its history file"))?;
    let registration = exchange::registration(&directory).map_err(failure)?;
    let (changed, mut changes) = tokio::sync::mpsc::channel(1);
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        if crate::file_events::may_change(&event) {
            let _ = changed.try_send(());
        }
    });
    watcher
        .watch(
            transcript.parent().ok_or_else(|| Failure::new("Claude transcript has no parent"))?,
            notify::RecursiveMode::NonRecursive,
        )
        .map_err(|error| Failure::new(error.to_string()))?;
    let process = registration
        .and_then(|registration| registration.process)
        .ok_or_else(|| Failure::new("Claude has not identified its process"))?;
    let exited = fastctx_platform::host_process::exited(&process);
    tokio::pin!(exited);
    let (tail, uuids, context_tail) = loop {
        let (reading, cuts) = (transcript.clone(), cuts.clone());
        match tokio::task::spawn_blocking(move || transcript::read_with_cuts(&reading, &cuts))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
        {
            Ok(read) if !read.pending_cuts => {
                if !read.problems.is_empty() {
                    return Err(Failure::new(read.problems.join("; ")));
                }
                let tail = read
                    .context_uuids
                    .last()
                    .cloned()
                    .ok_or_else(|| Failure::new("Claude has not persisted its replacement prefix"))?;
                break (tail, read.context_uuids, read.context_tail);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(failure(error)),
        }
        tokio::select! {
            Some(()) = changes.recv() => {},
            result = &mut exited => {
                result.map_err(failure)?;
                return Err(Failure::new("Claude exited before persisting its replacement prefix"));
            }
        }
    };
    node.claude.state.lock().expect("never poisoned").stored.insert(session.clone(), tail.clone());
    super::control::invoke(
        node,
        resident,
        json!({"do":"rebased", "last_uuid":tail, "uuids":uuids, "context_tail":context_tail}),
    )
    .await?;
    if let Some(from) = from {
        let record = crate::delivery::history(&node.world.conversation(resident)).map_err(failure)?;
        let previous: std::collections::HashSet<_> = record
            .iter()
            .flat_map(|event| match &event.happened {
                Happened::Changed { change: SessionChange::HistoryCut { removed_turns }, .. } => removed_turns.clone(),
                _ => Vec::new(),
            })
            .collect();
        let mut removing = false;
        let removed_turns: Vec<_> = record
            .iter()
            .filter_map(|event| {
                if let Happened::TurnStarted { turn } = &event.happened {
                    if turn == from {
                        removing = true
                    }
                    if removing {
                        return Some(turn.clone());
                    }
                }
                None
            })
            .collect();
        let last = record.iter().rev().find_map(|event| match &event.happened {
            Happened::TurnEnded { turn, end } if !removed_turns.contains(turn) && !previous.contains(turn) => {
                end.last_message.as_deref()
            }
            _ => None,
        });
        node.residents
            .change_now(&node.world, resident, |now| {
                now.last_said = last.map(|text| crate::turns::opening(text, fastctx_types::residents::LAST_SAID_CHARS));
            })
            .map_err(failure)?;
        crate::turns::apply(
            node,
            resident,
            SessionEvent {
                at: Timestamp::now(),
                happened: Happened::Changed { turn: None, change: SessionChange::HistoryCut { removed_turns } },
            },
        )
        .await
        .map_err(failure)?;
    }
    crate::turns::apply(
        node,
        resident,
        SessionEvent {
            at: Timestamp::now(),
            happened: Happened::Changed { turn: None, change: SessionChange::HistoryChanged },
        },
    )
    .await
    .map_err(failure)
}

fn failure(error: std::io::Error) -> Failure {
    Failure::new(error.to_string())
}

pub(super) fn cuts(directory: &std::path::Path) -> std::io::Result<Vec<transcript::HistoryCut>> {
    files::read(&directory.join("history-cuts.json")).map(|cuts| cuts.unwrap_or_default())
}
