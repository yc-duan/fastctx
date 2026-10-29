//! Threads Codex spawned as agents of a resident's thread (`multi_agent`):
//! what they do is told to the resident whose conversation spawned them,
//! as its agents' events (`SessionChange::AgentEvent`), so whoever watches
//! the conversation watches its agents work.
//!
//! The app-server tells every connection of a thread it spawns, and from
//! then on of all it does, under the spawned thread's own id; it says whose
//! agent the thread is only when asked (`thread/read`). A thread nobody
//! claims is asked about once; what it reports meanwhile waits, in order,
//! and goes to the resident when the answer names one, or the way of any
//! other thread nobody claims when it does not. One no answer can be had
//! about is asked again a while after, its reports still waiting, so an
//! agent that ended while its app-server did not answer is still told.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_hosts::codex::{HostEvent, Spawned};
use fastctx_types::session::{Happened, HostAgentStart, SessionChange, SessionEvent, TurnEnd, TurnHow};

use crate::Node;

/// How many spawners up a thread is followed to the resident's thread it
/// belongs to: Codex refuses a spawn deeper than its `agent_max_depth`, 1
/// by default; this bounds a walk through what the app-server answers.
const DEEPEST: usize = 16;

#[derive(Default)]
pub struct Agents {
    threads: Mutex<HashMap<String, Thread>>,
}

enum Thread {
    /// Being asked about: what it reported meanwhile, in order, and since
    /// when no answer could be had, if it was asked before.
    Asking { waiting: Vec<HostEvent>, unknown: Option<Instant> },
    /// An agent of the resident whose thread is `root`.
    Agent { root: String, model: Option<String> },
    /// No resident's agent.
    Other,
    /// Asking failed, as when its app-server did not answer, first `since`:
    /// what it reports meanwhile waits, and it is asked again once a while
    /// has passed, whether or not it reports more. Past `GIVE_UP`, what it
    /// reports goes the way of any thread nobody claims, and it is asked
    /// again only when it reports.
    Unknown { asked: Instant, since: Instant, waiting: Vec<HostEvent> },
}

/// How long a thread asking about failed waits before it is asked again.
const ASK_AGAIN: Duration = Duration::from_secs(30);
/// How long the reports of a thread no answer can be had about wait.
const GIVE_UP: Duration = Duration::from_secs(60 * 60);
/// The most reports of such a thread that wait; later ones are let go.
const WAITING: usize = 4096;

/// What asking about a thread found.
enum Found {
    /// An agent of the resident whose thread is the first, below the agent
    /// that is the second where another agent spawned it.
    Agent(String, Option<String>, Spawned),
    /// No resident's agent.
    Other,
    /// No answer to be had now.
    Unknown,
}

/// Takes what a thread nobody claims reported, when the thread is, or may
/// be, an agent of a resident's thread; hands it back otherwise, to go the
/// way of any thread nobody claims (`plain`).
pub(super) fn took(
    node: &Arc<Node>,
    thread: &str,
    event: HostEvent,
    plain: fn(&Arc<Node>, String, HostEvent) -> std::io::Result<()>,
) -> Option<HostEvent> {
    let mut threads = node.codex_agents.threads.lock().expect("never poisoned");
    match threads.get_mut(thread) {
        Some(Thread::Other) => Some(event),
        // Given up on: it goes as before, asked again only as it reports.
        Some(Thread::Unknown { asked, since, .. }) if since.elapsed() >= GIVE_UP => {
            if asked.elapsed() < ASK_AGAIN {
                return Some(event);
            }
            let since = *since;
            threads.insert(thread.to_string(), Thread::Asking { waiting: vec![event], unknown: Some(since) });
            ask(node, thread, plain);
            None
        }
        Some(Thread::Unknown { asked, waiting, .. }) if asked.elapsed() < ASK_AGAIN => {
            if waiting.len() < WAITING {
                waiting.push(event);
            }
            None
        }
        Some(Thread::Asking { waiting, .. }) => {
            waiting.push(event);
            None
        }
        Some(Thread::Agent { root, model }) => {
            let root = root.clone();
            tell(node, &root, thread, model.as_deref(), event, plain);
            None
        }
        Some(Thread::Unknown { .. }) | None => {
            let asking = match threads.remove(thread) {
                Some(Thread::Unknown { since, mut waiting, .. }) => {
                    waiting.push(event);
                    Thread::Asking { waiting, unknown: Some(since) }
                }
                _ => Thread::Asking { waiting: vec![event], unknown: None },
            };
            threads.insert(thread.to_string(), asking);
            ask(node, thread, plain);
            None
        }
    }
}

/// Asks whose `thread`, now being asked about, is, and settles it.
fn ask(node: &Arc<Node>, thread: &str, plain: fn(&Arc<Node>, String, HostEvent) -> std::io::Result<()>) {
    let (node, thread) = (node.clone(), thread.to_string());
    tokio::spawn(async move {
        let found = whose(&node, &thread).await;
        settle(&node, &thread, found, plain);
    });
}

/// Asks again about `thread` a while after no answer could be had, unless
/// a report of it had it asked meanwhile.
fn again(node: &Arc<Node>, thread: &str, plain: fn(&Arc<Node>, String, HostEvent) -> std::io::Result<()>) {
    let mut threads = node.codex_agents.threads.lock().expect("never poisoned");
    let due = matches!(
        threads.get(thread),
        Some(Thread::Unknown { asked, since, .. }) if asked.elapsed() >= ASK_AGAIN && since.elapsed() < GIVE_UP
    );
    if !due {
        return;
    }
    let Some(Thread::Unknown { since, waiting, .. }) = threads.remove(thread) else { return };
    threads.insert(thread.to_string(), Thread::Asking { waiting, unknown: Some(since) });
    drop(threads);
    ask(node, thread, plain);
}

/// The answer about a thread came: what it reported meanwhile goes on, in
/// order, before anything it reports after, which waits on the same lock.
fn settle(
    node: &Arc<Node>,
    thread: &str,
    found: Found,
    plain: fn(&Arc<Node>, String, HostEvent) -> std::io::Result<()>,
) {
    let mut threads = node.codex_agents.threads.lock().expect("never poisoned");
    let Some(Thread::Asking { waiting, unknown }) = threads.remove(thread) else { return };
    match found {
        Found::Agent(root, parent, spawned) => {
            threads.insert(thread.to_string(), Thread::Agent { root: root.clone(), model: spawned.model.clone() });
            let at = waiting.iter().find_map(|event| match event {
                HostEvent::Happened { event, .. } => Some(event.at),
                _ => None,
            });
            let started = SessionEvent {
                at: at.unwrap_or_else(jiff::Timestamp::now),
                happened: Happened::Changed {
                    turn: None,
                    change: SessionChange::AgentStarted {
                        agent: Box::new(HostAgentStart {
                            id: thread.to_string(),
                            parent,
                            kind: spawned.role,
                            name: spawned.nickname,
                            model: spawned.model.clone(),
                            background: true,
                            live: true,
                            ..Default::default()
                        }),
                    },
                },
            };
            if let Err(error) = plain(node, root.clone(), HostEvent::Happened { thread: root.clone(), event: started })
            {
                node.log.line(format_args!("telling of the agent {thread}: {error}"));
            }
            for event in waiting {
                tell(node, &root, thread, spawned.model.as_deref(), event, plain);
            }
        }
        // Its reports wait, and it is asked again a while after, whether or
        // not it reports more.
        Found::Unknown if unknown.is_none_or(|since| since.elapsed() < GIVE_UP) => {
            let since = unknown.unwrap_or_else(Instant::now);
            threads.insert(thread.to_string(), Thread::Unknown { asked: Instant::now(), since, waiting });
            drop(threads);
            let (node, thread) = (node.clone(), thread.to_string());
            tokio::spawn(async move {
                tokio::time::sleep(ASK_AGAIN).await;
                again(&node, &thread, plain);
            });
        }
        Found::Other | Found::Unknown => {
            let settled = match found {
                Found::Unknown => Thread::Unknown {
                    asked: Instant::now(),
                    since: unknown.unwrap_or_else(Instant::now),
                    waiting: Vec::new(),
                },
                _ => Thread::Other,
            };
            threads.insert(thread.to_string(), settled);
            for event in waiting {
                if let Err(error) = plain(node, thread.to_string(), event) {
                    node.log.line(format_args!("retaining a Codex report: {error}"));
                }
            }
        }
    }
}

/// What the agent `agent` reported, told to the resident whose thread is
/// `root` as one of its agents' events.
fn tell(
    node: &Arc<Node>,
    root: &str,
    agent: &str,
    model: Option<&str>,
    event: HostEvent,
    plain: fn(&Arc<Node>, String, HostEvent) -> std::io::Result<()>,
) {
    let told: Vec<SessionEvent> = match &event {
        HostEvent::Happened { event, .. } => vec![event.clone()],
        // Its app-server stopped under its turn.
        HostEvent::Gone { how, turns, .. } => turns
            .iter()
            .map(|(_, turn)| SessionEvent {
                at: jiff::Timestamp::now(),
                happened: Happened::TurnEnded {
                    turn: turn.clone(),
                    end: TurnEnd {
                        how: TurnHow::Interrupted,
                        reason: Some(how.clone()),
                        error: None,
                        duration_ms: None,
                        facts: None,
                        last_message: None,
                    },
                },
            })
            .collect(),
        // The desk and the loading of an agent's thread are its own.
        HostEvent::Desk { .. } | HostEvent::Unloaded { .. } | HostEvent::Synchronized { .. } => Vec::new(),
    };
    node.codex.acknowledge(&event);
    if told.iter().any(|event| matches!(event.happened, Happened::TurnEnded { .. })) {
        node.watchers.changed.notify_one();
    }
    for mut event in told {
        if let Happened::Changed { change: SessionChange::Usage { usage }, .. } = &mut event.happened
            && let Some(counted) = usage.get_mut("counted")
            && counted.get("model").is_none_or(serde_json::Value::is_null)
        {
            counted["model"] = serde_json::json!(model);
        }
        let wrapped = SessionEvent {
            at: event.at,
            happened: Happened::Changed {
                turn: None,
                change: SessionChange::AgentEvent { agent_id: agent.to_string(), happened: Box::new(event.happened) },
            },
        };
        if let Err(error) =
            plain(node, root.to_string(), HostEvent::Happened { thread: root.to_string(), event: wrapped })
        {
            node.log.line(format_args!("telling what the agent {agent} did: {error}"));
        }
    }
}

/// The resident's thread `thread` is an agent of, and the agent above it
/// where another agent spawned it. The app-server that spawned it answers;
/// the others do not know it. One that could not be asked leaves it unknown,
/// not another's.
async fn whose(node: &Arc<Node>, thread: &str) -> Found {
    let mut unanswered = false;
    for host in node.codex.all() {
        let spawned = match host.spawned_by(thread).await {
            Ok(Some(spawned)) => spawned,
            Ok(None) => continue,
            Err(_) => {
                unanswered = true;
                continue;
            }
        };
        let mut above = spawned.parent.clone();
        for _ in 0..DEEPEST {
            if node.residents.resident_of_thread(&above).is_some() {
                let parent = (above != spawned.parent).then(|| spawned.parent.clone());
                return Found::Agent(above, parent, spawned);
            }
            let known = match node.codex_agents.threads.lock().expect("never poisoned").get(&above) {
                Some(Thread::Agent { root, .. }) => Some(root.clone()),
                _ => None,
            };
            if let Some(root) = known {
                return Found::Agent(root, Some(spawned.parent.clone()), spawned);
            }
            match host.spawned_by(&above).await {
                Ok(Some(up)) => above = up.parent,
                Ok(None) => return Found::Other,
                Err(_) => return Found::Unknown,
            }
        }
        return Found::Other;
    }
    if unanswered { Found::Unknown } else { Found::Other }
}
