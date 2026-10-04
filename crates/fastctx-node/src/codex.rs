//! This machine's Codex app-server, as the node holds it
//! (`world-colleagues.md`, 世界怎样拉起 GPT 同事).
//!
//! When the node starts, the threads of the Codex residents the World
//! started are made known to the host again, each resumed when words next
//! go to it. Whatever the host reports of a thread goes to the resident
//! that thread is.

use std::collections::HashMap;
use std::sync::Arc;

use fastctx_hosts::codex::HostEvent;
use fastctx_types::ResidentId;
use fastctx_types::residents::{Activity, Origin};
use fastctx_types::session::{Happened, SessionEvent};
use tokio::sync::mpsc;

use crate::Node;

/// Makes the World's own Codex threads known to the host again. Those idle
/// whose asker is gone are asleep: the node that kept them is gone too.
pub async fn adopt(node: &Arc<Node>) {
    let here = node.machine();
    for resident in node.residents.everyone() {
        let about = &resident.about;
        if !about.world_started {
            continue;
        }
        // What it was told of its town when it was started, it still has.
        match crate::colleagues::heard_now(node, resident.town.as_ref()) {
            Ok(heard) => node.residents.set_heard(&about.id, heard),
            Err(error) => node.log.line(format_args!("reading {}'s town: {error}", about.id)),
        }
        if about.host.agent.as_str() != "codex" {
            continue;
        }
        let Some(thread) = &about.session else { continue };
        match crate::colleagues::thread_spec(node, &about.id, &about.origin, resident.town.as_ref()).await {
            Ok((spec, _)) => node.codex.adopt(thread, spec),
            Err(failure) => {
                node.log.line(format_args!("{} cannot be resumed: {failure}", about.id));
                continue;
            }
        }
        let asleep = match &about.origin {
            Origin::Colleague { asked_by, .. } => !node.residents.requester_here(asked_by, &here),
            Origin::User => false,
        };
        if asleep && resident.now.as_ref().is_some_and(|now| now.activity == Activity::Idle) {
            let slept = node.residents.change_now(&node.world, &about.id, |now| now.activity = Activity::Asleep);
            if let Err(error) = slept {
                node.log.line(format_args!("recording that {} sleeps: {error}", about.id));
            }
        }
    }
}

/// Reports are queued before dispatch, including Desk and Gone before a
/// thread is claimed. A separate worker serializes each thread; a slow
/// submission never prevents another thread's reports reaching disk.
///
/// Only what the World answers for is kept on disk while it waits: how a
/// turn starts and ends, what is wrong with a session, its desk and its
/// app-server's end. What the agent did is in the thread's own record,
/// and what grows an item is for those watching live; were the node to
/// stop, its app-server stops with it and nobody watches.
#[derive(Default)]
pub struct Unclaimed {
    state: std::sync::Mutex<Reports>,
}

#[derive(Default)]
struct Reports {
    queues: HashMap<String, Vec<HostEvent>>,
    claimed: std::collections::HashSet<String>,
    workers: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct KeptReports {
    thread: String,
    events: Vec<HostEvent>,
}

/// Whether a report is kept on disk until it is applied.
fn durable(event: &HostEvent) -> bool {
    match event {
        HostEvent::Happened { event, .. } => matches!(
            event.happened,
            Happened::TurnStarted { .. } | Happened::TurnEnded { .. } | Happened::Health { .. }
        ),
        HostEvent::Desk { .. } | HostEvent::Gone { .. } => true,
    }
}

fn save(node: &Node, thread: &str, events: &[HostEvent]) -> std::io::Result<()> {
    let directory = node.world.path().join("codex-reports");
    fastctx_platform::ensure_private_dir(&directory)?;
    let key = crate::speech::fixed_id("thread", &[thread]);
    let events = events.iter().filter(|event| durable(event)).cloned().collect();
    let kept = KeptReports { thread: thread.to_string(), events };
    fastctx_platform::publish(
        &directory.join(format!("{key}.json")),
        &serde_json::to_vec(&kept).map_err(std::io::Error::other)?,
        fastctx_platform::Permissions::Private,
    )
}

fn enqueue(node: &Arc<Node>, thread: String, event: HostEvent) -> std::io::Result<()> {
    {
        let mut state = node.unclaimed.state.lock().expect("never poisoned");
        let events = state.queues.entry(thread.clone()).or_default();
        let kept = durable(&event);
        events.push(event);
        if kept {
            save(node, &thread, events)?;
        }
    }
    let node = node.clone();
    tokio::spawn(async move {
        drain(&node, &thread).await;
    });
    Ok(())
}

async fn drain(node: &Arc<Node>, thread: &str) {
    let worker = {
        let mut state = node.unclaimed.state.lock().expect("never poisoned");
        if !state.claimed.contains(thread) {
            return;
        }
        state.workers.entry(thread.to_string()).or_default().clone()
    };
    let _worker = worker.lock().await;
    loop {
        let event = node
            .unclaimed
            .state
            .lock()
            .expect("never poisoned")
            .queues
            .get(thread)
            .and_then(|events| events.first())
            .cloned();
        let Some(event) = event else { return };
        let Some(id) = node.residents.resident_of_thread(thread) else { return };
        let result = match event {
            HostEvent::Happened { event, .. } => crate::turns::apply(node, &id, event).await,
            HostEvent::Desk { turn, desk, .. } => {
                let town = node.residents.town_of(&id).flatten();
                let directory = node.world.resident(town.as_ref(), &id);
                match crate::delivery::terminal(&directory, &turn) {
                    Ok(None) => {
                        crate::desk::used(node, &id, &turn, desk);
                        Ok(())
                    }
                    Ok(Some(_)) => Ok(()),
                    Err(error) => Err(error),
                }
            }
            HostEvent::Gone { how, turns } => {
                let mut result = Ok(());
                for (_, turn) in turns {
                    let event = SessionEvent {
                        at: jiff::Timestamp::now(),
                        happened: fastctx_types::session::Happened::TurnEnded {
                            turn,
                            end: fastctx_types::session::TurnEnd {
                                how: fastctx_types::session::TurnHow::Interrupted,
                                reason: Some(format!("its app-server stopped ({how})")),
                                duration_ms: None,
                                last_message: None,
                            },
                        },
                    };
                    if let Err(error) = crate::turns::apply(node, &id, event).await {
                        result = Err(error);
                        break;
                    }
                }
                result
            }
        };
        if let Err(error) = result {
            node.log.line(format_args!("applying a retained report for {thread}: {error}"));
            return;
        }
        let mut state = node.unclaimed.state.lock().expect("never poisoned");
        let events = state.queues.get_mut(thread).expect("the worker owns its queue");
        if durable(&events[0])
            && let Err(error) = save(node, thread, &events[1..])
        {
            node.log.line(format_args!("acknowledging a report for {thread}: {error}"));
            return;
        }
        events.remove(0);
    }
}

/// Restores report queues before hearing any more from the new app-server.
pub fn listen(node: Arc<Node>, mut events: mpsc::UnboundedReceiver<HostEvent>) {
    {
        let mut state = node.unclaimed.state.lock().expect("never poisoned");
        for resident in node.residents.everyone() {
            if let Some(thread) = resident.about.session {
                state.claimed.insert(thread);
            }
        }
        match std::fs::read_dir(node.world.path().join("codex-reports")) {
            Ok(entries) => {
                for entry in entries {
                    let restored = (|| -> std::io::Result<KeptReports> {
                        let path = entry?.path();
                        serde_json::from_slice(&std::fs::read(path)?).map_err(std::io::Error::other)
                    })();
                    match restored {
                        Ok(kept) => {
                            state.queues.insert(kept.thread, kept.events);
                        }
                        Err(error) => node.log.line(format_args!("restoring Codex reports: {error}")),
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => node.log.line(format_args!("restoring Codex reports: {error}")),
        }
    }
    let threads: Vec<String> = node.unclaimed.state.lock().expect("never poisoned").queues.keys().cloned().collect();
    for thread in threads {
        let node = node.clone();
        tokio::spawn(async move {
            drain(&node, &thread).await;
        });
    }
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let reports = match event {
                HostEvent::Gone { how, turns } => turns
                    .into_iter()
                    .map(|(thread, turn)| {
                        (thread.clone(), HostEvent::Gone { how: how.clone(), turns: vec![(thread, turn)] })
                    })
                    .collect(),
                event @ (HostEvent::Happened { .. } | HostEvent::Desk { .. }) => {
                    let thread = match &event {
                        HostEvent::Happened { thread, .. } | HostEvent::Desk { thread, .. } => thread.clone(),
                        _ => unreachable!(),
                    };
                    vec![(thread, event)]
                }
            };
            for (thread, event) in reports {
                if let Err(error) = enqueue(&node, thread, event) {
                    node.log.line(format_args!("retaining a Codex report: {error}"));
                }
            }
        }
    });
}

/// A resident whose thread the World holds moved to another town: when its
/// thread is next resumed, it is told it lives there.
pub async fn moved(node: &Arc<Node>, id: &ResidentId) {
    let Some((about, town)) = node.residents.about(id) else { return };
    if !about.world_started || about.host.agent.as_str() != "codex" {
        return;
    }
    let Some(thread) = &about.session else { return };
    match crate::colleagues::thread_spec(node, id, &about.origin, town.as_ref()).await {
        Ok((spec, _)) => node.codex.adopt(thread, spec),
        Err(failure) => node.log.line(format_args!("{id} moved, and its thread cannot be told: {failure}")),
    }
}

/// The resident `id` is the thread `thread` now: what was reported of the
/// thread before goes to it, in order, before anything reported after.
pub async fn claim(node: &Arc<Node>, thread: &str, _id: &ResidentId) {
    node.unclaimed.state.lock().expect("never poisoned").claimed.insert(thread.to_string());
    drain(node, thread).await;
}

pub async fn replay(node: &Arc<Node>) {
    let threads: Vec<String> = node.unclaimed.state.lock().expect("never poisoned").queues.keys().cloned().collect();
    for thread in threads {
        drain(node, &thread).await;
    }
}
