//! This machine's Codex app-server, as the node holds it
//! (`world-colleagues.md`, 世界怎样拉起 GPT 同事).
//!
//! When the node starts, the threads of the Codex residents the World
//! started are made known to the host again, each resumed when words next
//! go to it. Whatever the host reports of a thread goes to the resident
//! that thread is.

use std::sync::Arc;

use fastctx_hosts::codex::HostEvent;
use fastctx_types::ResidentId;

use fastctx_types::session::SessionEvent;
use tokio::sync::mpsc;

use crate::Node;

pub mod control;
mod directories;
pub mod hosts;
pub(crate) mod native;
pub(crate) mod native_context;
pub(crate) mod native_lifetime;
pub(crate) mod report_store;
mod reports;
pub mod spawned;
pub(crate) mod submitted;
pub mod watch;

/// Makes the World's own Codex threads known to the host again. Idle
/// sessions start asleep: this node has not loaded their runtimes yet. One
/// whose preparation failed, as when its town could not be read for a
/// moment, is tried again each minute for an hour.
pub async fn adopt(node: &Arc<Node>, residents: &[crate::residents::OnRecord]) {
    let mut failed = Vec::new();
    for resident in residents {
        if !adopt_one(node, resident, true).await {
            failed.push(resident.about.id.clone());
        }
    }
    if failed.is_empty() {
        return;
    }
    let node = Arc::downgrade(node);
    fastctx_platform::task::spawn(async move {
        for _ in 0..60 {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            let Some(node) = node.upgrade() else { return };
            let mut left = Vec::new();
            for id in failed {
                let Some(resident) = node.residents.everyone().find(|resident| resident.about.id == id) else {
                    continue;
                };
                if !adopt_one(&node, &resident, false).await {
                    left.push(id);
                }
            }
            if left.is_empty() {
                return;
            }
            failed = left;
        }
    });
}

/// Makes one resident's Codex thread known to its host again; false where
/// that failed and may succeed later. A failure is said only on the first
/// try.
async fn adopt_one(node: &Arc<Node>, resident: &crate::residents::OnRecord, first: bool) -> bool {
    let about = &resident.about;
    let say = |what: std::fmt::Arguments| {
        if first {
            node.log.line(what);
        }
    };
    if about.native_resumed && resident.removed.is_none() {
        if let Some(thread) = &about.session {
            match node.codex.of(node, about) {
                Ok(host) => host.adopt(thread, native::spec(node, about)),
                Err(error) => {
                    say(format_args!("{} cannot be resumed: {error}", about.id));
                    return false;
                }
            }
        }
        crate::lifecycle::observed(node, &about.id, false, "node restarted");
        return true;
    }
    if !about.world_started || resident.removed.is_some() {
        return true;
    }
    // What it was told of its town when it was started, it still has.
    match crate::colleagues::heard_now(node, resident.town.as_ref()) {
        Ok(heard) => node.residents.set_instructed(&about.id, heard),
        Err(error) => say(format_args!("reading {}'s town: {error}", about.id)),
    }
    if about.host.agent.as_str() != "codex" {
        return true;
    }
    let Some(thread) = &about.session else { return true };
    match crate::colleagues::thread_spec(
        node,
        &about.id,
        &about.origin,
        resident.town.as_ref(),
        &about.acting,
        about.home.as_deref(),
    )
    .await
    {
        Ok((spec, _)) => match node.codex.of(node, about) {
            Ok(codex) => codex.adopt(thread, spec),
            Err(failure) => {
                say(format_args!("{} cannot be resumed: {failure}", about.id));
                return false;
            }
        },
        Err(failure) => {
            say(format_args!("{} cannot be resumed: {failure}", about.id));
            return false;
        }
    }
    crate::lifecycle::observed(node, &about.id, false, "node restarted");
    true
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
///
/// A thread nobody claims may still be one the World is starting or forking,
/// whose reports can come before Codex answers which thread it is; its
/// reports are kept for as long as such a start is unanswered. Once none is,
/// a thread nobody claimed is none of the World's, such as a subagent Codex
/// itself spawned: its reports go, and the World keeps none of its text.
#[derive(Default)]
pub struct Unclaimed {
    state: std::sync::Mutex<reports::State>,
    persisting: crate::leases::Registry<String, tokio::sync::Mutex<()>>,
    workers: crate::leases::Registry<String, fastctx_platform::doing::Mutex<()>>,
    starting: std::sync::Mutex<usize>,
}

/// Held while what a thread reported is applied (`drain`).
pub(crate) fn working(node: &Node, thread: &str) -> crate::leases::Lease<String, fastctx_platform::doing::Mutex<()>> {
    node.unclaimed.workers.get_or_insert_with(&thread.to_string(), || crate::ranked::working(thread))
}

/// Its conversation was deleted: what its thread reported and is still
/// kept goes with it.
pub(crate) async fn deleted(
    node: &Arc<Node>,
    thread: &str,
    directory: &fastctx_types::layout::ConversationDir,
) -> std::io::Result<()> {
    let gate = node.unclaimed.persisting.get_or_insert_with(&thread.to_string(), || tokio::sync::Mutex::new(()));
    let _persisting = gate.lock().await;
    let (directory, thread) = (directory.clone(), thread.to_string());
    let deleting = thread.clone();
    fastctx_platform::task::spawn_blocking(move || report_store::delete(&directory, &deleting))
        .await
        .map_err(std::io::Error::other)??;
    node.unclaimed.state.lock().expect("host reports").queues.remove(&thread);
    Ok(())
}

pub(crate) fn pending(
    node: &Node,
    directory: &fastctx_types::layout::ConversationDir,
    thread: Option<&str>,
) -> std::io::Result<bool> {
    if let Some(thread) = thread {
        if reports::pending(node, thread) {
            return Ok(true);
        }
        return report_store::pending_thread(node, thread);
    }
    if report_store::pending(directory)? {
        return Ok(true);
    }
    Ok(fastctx_platform::disk::exists(directory.books().join("host-reports.json"))?
        || fastctx_platform::disk::exists(directory.books().join("host-reports.jsonl"))?)
}

/// A thread the World is starting or forking, until it is claimed or the
/// start failed.
pub struct Starting {
    node: Arc<Node>,
}

/// Holds reports of threads nobody claims until the thread started now is
/// claimed: keep it from before Codex is asked until after `claim`.
pub fn starting(node: &Arc<Node>) -> Starting {
    *node.unclaimed.starting.lock().expect("never poisoned") += 1;
    Starting { node: node.clone() }
}

impl Drop for Starting {
    fn drop(&mut self) {
        let none = {
            let mut starting = self.node.unclaimed.starting.lock().expect("never poisoned");
            *starting -= 1;
            *starting == 0
        };
        if none {
            let mut state = self.node.unclaimed.state.lock().expect("host reports");
            if *self.node.unclaimed.starting.lock().expect("host starting") == 0 {
                state.queues.retain(|thread, _| self.node.residents.resident_of_thread(thread).is_some());
            }
        }
    }
}

/// Whether a report is kept on disk until it is applied.
fn durable(event: &HostEvent) -> bool {
    match event {
        HostEvent::Synchronized { .. } => false,
        HostEvent::Happened { event, .. } => event.happened.kept(),
        HostEvent::Unloaded { .. } | HostEvent::Desk { .. } | HostEvent::Gone { .. } => true,
    }
}

/// A thread nobody claims may be an agent Codex spawned for a resident's
/// thread, whose reports go to that resident (`spawned`).
fn enqueue(node: &Arc<Node>, thread: String, event: HostEvent) -> std::io::Result<()> {
    if node.residents.resident_of_thread(&thread).is_some() {
        return plain(node, thread, event);
    }
    match spawned::took(node, &thread, event, plain) {
        None => Ok(()),
        Some(event) => plain(node, thread, event),
    }
}

fn plain(node: &Arc<Node>, thread: String, event: HostEvent) -> std::io::Result<()> {
    let (flush, drain) = reports::push(node, thread.clone(), event);
    if flush {
        fastctx_platform::task::spawn(reports::persist(node.clone(), thread.clone()));
    }
    if drain {
        let node = node.clone();
        fastctx_platform::task::spawn(async move {
            self::drain(&node, &thread).await;
        });
    }
    Ok(())
}

async fn drain(node: &Arc<Node>, thread: &str) {
    // A thread whose conversation is not found now keeps what it reported,
    // in memory and on disk: the next report or the next start drains it.
    if node.residents.resident_of_thread(thread).is_none() {
        reports::failed(node, thread);
        return;
    }
    let _doing = fastctx_platform::doing::doing(format!("reading the reports of Codex thread {thread}"));
    let worker = working(node, thread);
    let _worker = worker.lock().await;
    if node.residents.resident_of_thread(thread).is_none() {
        reports::failed(node, thread);
        return;
    }
    loop {
        if let Err(error) = reports::flush(node, thread).await {
            reports::failed(node, thread);
            node.log.line(format_args!("restoring reports for {thread}: {error}"));
            return;
        }
        let selected = match reports::next(node, thread).await {
            Ok(reports::Next::Event(selected)) => *selected,
            Ok(reports::Next::More) => continue,
            Ok(reports::Next::Empty) => return,
            Err(error) => {
                reports::failed(node, thread);
                node.log.line(format_args!("reading reports for {thread}: {error}"));
                return;
            }
        };
        let event = selected.event.clone();
        let Some(id) = node.residents.resident_of_thread(thread) else {
            reports::failed(node, thread);
            return;
        };
        let result =
            if selected.applied {
                Ok(())
            } else {
                match event {
                    HostEvent::Synchronized { .. } => unreachable!("barriers do not enter the durable queue"),
                    HostEvent::Unloaded { .. } => {
                        if node.residents.about(&id).is_some_and(|(about, _)| {
                            !node.codex.of(node, &about).is_ok_and(|codex| codex.loaded(thread))
                        }) {
                            crate::lifecycle::observed(node, &id, false, "thread unloaded");
                            Ok(())
                        } else {
                            Ok(())
                        }
                    }
                    HostEvent::Happened { event, .. } => match control::reported(node, &id, thread, &event) {
                        Ok(()) => match crate::user_images::codex(node, &id, thread, event).await {
                            Ok(events) => {
                                let mut result = Ok(());
                                for event in events {
                                    if let Err(error) = crate::turns::apply(node, &id, event).await {
                                        result = Err(error);
                                        break;
                                    }
                                }
                                result
                            }
                            Err(error) => Err(error),
                        },
                        Err(error) => Err(error),
                    },
                    HostEvent::Desk { turn, desk, .. } => {
                        let directory = node.world.conversation(&id);
                        match crate::delivery::terminal(&directory, &turn) {
                            Ok(None) => {
                                crate::desk::used(node, &id, &turn, desk);
                                Ok(())
                            }
                            Ok(Some(_)) => Ok(()),
                            Err(error) => Err(error),
                        }
                    }
                    HostEvent::Gone { how, turns, facts } => {
                        let mut result = Ok(());
                        for (thread, turn) in turns {
                            let facts = facts
                                .iter()
                                .find(|(t, id, _)| t == &thread && id == &turn)
                                .map(|(_, _, facts)| facts.clone());
                            let event = SessionEvent {
                                at: jiff::Timestamp::now(),
                                happened: fastctx_types::session::Happened::TurnEnded {
                                    turn,
                                    end: fastctx_types::session::TurnEnd {
                                        facts: facts.map(Box::new),
                                        error: Some(serde_json::json!({"world":"host_exited"})),
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
                }
            };
        if let Err(error) = result {
            reports::failed(node, thread);
            node.log.line(format_args!("applying a retained report for {thread}: {error}"));
            return;
        }
        reports::applied(node, thread, &selected);
        if let Err(error) = reports::acknowledge(node, thread, &selected).await {
            reports::failed(node, thread);
            node.log.line(format_args!("acknowledging a report for {thread}: {error}"));
            return;
        }
        let applied = selected.event;
        node.codex.acknowledge(&applied);
        if matches!(
            &applied,
            HostEvent::Happened {
                event: SessionEvent { happened: fastctx_types::session::Happened::TurnEnded { .. }, .. },
                ..
            } | HostEvent::Gone { .. }
        ) {
            node.watchers.changed.tell(&id);
        }
        node.handoff.changed();
    }
}

/// Restores report queues before hearing any more from the new app-server.
/// Nothing is being started as the node starts: what was kept of a thread
/// nobody claimed goes.
pub fn listen(node: Arc<Node>, mut events: mpsc::UnboundedReceiver<HostEvent>) {
    let threads: Vec<_> = node
        .residents
        .everyone()
        .filter_map(|resident| {
            (resident.about.host.agent.as_str() == "codex").then_some(resident.about.session).flatten()
        })
        .collect();
    for thread in threads {
        let node = node.clone();
        fastctx_platform::task::spawn(async move {
            drain(&node, &thread).await;
        });
    }
    let cleaning = node.clone();
    fastctx_platform::task::spawn_blocking(move || {
        if let Err(error) = report_store::discard_legacy(&cleaning) {
            cleaning.log.line(format_args!("removing reports of threads nobody claimed: {error}"));
        }
    });
    fastctx_platform::task::spawn(async move {
        while let Some(event) = events.recv().await {
            let reports = match event {
                HostEvent::Synchronized { thread, done } => {
                    // In a task of its own: what goes wrong with one thread
                    // stays with it, and listening goes on.
                    let (draining, drained) = (node.clone(), thread.clone());
                    if let Err(error) =
                        fastctx_platform::task::spawn(async move { drain(&draining, &drained).await }).await
                    {
                        node.log.line(format_args!("applying what {thread} reported: {error}"));
                    }
                    let applied = match reports::empty(&node, &thread).await {
                        Ok(true) => Ok(()),
                        Ok(false) => Err("some Codex events could not be applied; see the node log".to_string()),
                        Err(error) => Err(error.to_string()),
                    };
                    if let Some(done) = done.lock().expect("never poisoned").take() {
                        let _ = done.send(applied);
                    }
                    continue;
                }
                HostEvent::Gone { how, turns, facts } => turns
                    .into_iter()
                    .map(|(thread, turn)| {
                        let observed = facts.iter().filter(|(t, id, _)| t == &thread && id == &turn).cloned().collect();
                        (
                            thread.clone(),
                            HostEvent::Gone { how: how.clone(), turns: vec![(thread, turn)], facts: observed },
                        )
                    })
                    .collect(),
                event @ (HostEvent::Happened { .. } | HostEvent::Desk { .. } | HostEvent::Unloaded { .. }) => {
                    let thread = match &event {
                        HostEvent::Happened { thread, .. }
                        | HostEvent::Desk { thread, .. }
                        | HostEvent::Unloaded { thread } => thread.clone(),
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

/// A name given to a conversation's thread, kept at once: Codex tells of it
/// only to a thread it is following, which one at rest after its turn is not.
pub(crate) async fn titled(node: &Arc<Node>, id: &ResidentId, title: String) {
    let event = fastctx_types::session::SessionEvent {
        at: jiff::Timestamp::now(),
        happened: fastctx_types::session::Happened::Titled { title },
    };
    if let Err(error) = crate::turns::apply(node, id, event).await {
        node.log.line(format_args!("keeping {id}'s new name: {error}"));
    }
}

/// A resident whose thread the World holds moved to another town: when its
/// thread is next resumed, it is told it lives there.
pub async fn moved(node: &Arc<Node>, id: &ResidentId) {
    let Some((about, town)) = node.residents.about(id) else { return };
    if !about.world_started || about.host.agent.as_str() != "codex" {
        return;
    }
    let Some(thread) = &about.session else { return };
    match crate::colleagues::thread_spec(node, id, &about.origin, town.as_ref(), &about.acting, about.home.as_deref())
        .await
    {
        Ok((spec, _)) => match node.codex.of(node, &about) {
            Ok(codex) => codex.adopt(thread, spec),
            Err(failure) => node.log.line(format_args!("{id} moved, and its thread cannot be told: {failure}")),
        },
        Err(failure) => node.log.line(format_args!("{id} moved, and its thread cannot be told: {failure}")),
    }
}

/// The resident `id` is the thread `thread` now: what was reported of the
/// thread before goes to it, in order, before anything reported after.
pub async fn claim(node: &Arc<Node>, thread: &str, _id: &ResidentId) {
    drain(node, thread).await;
}

pub(crate) async fn replay_thread(node: &Arc<Node>, thread: &str) {
    drain(node, thread).await;
}

pub async fn replay(node: &Arc<Node>) {
    let mut threads: std::collections::BTreeSet<String> =
        node.unclaimed.state.lock().expect("never poisoned").queues.keys().cloned().collect();
    // A failed restore has no in-memory queue yet. Its durable reports must
    // still be revisited even if the host never sends another event.
    threads.extend(node.residents.everyone().filter_map(|resident| {
        (resident.about.host.agent.as_str() == "codex").then_some(resident.about.session).flatten()
    }));
    for thread in threads {
        drain(node, &thread).await;
    }
}
