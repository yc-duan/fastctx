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

use fastctx_types::session::SessionEvent;
use tokio::sync::mpsc;

use crate::Node;

pub mod control;
mod directories;
pub mod hosts;
pub(crate) mod native;
pub(crate) mod native_context;
pub(crate) mod native_lifetime;
pub mod spawned;
pub(crate) mod submitted;
pub mod watch;

/// Makes the World's own Codex threads known to the host again. Idle
/// sessions start asleep: this node has not loaded their runtimes yet. One
/// whose preparation failed, as when its town could not be read for a
/// moment, is tried again each minute for an hour.
pub async fn adopt(node: &Arc<Node>) {
    let mut failed = Vec::new();
    for resident in node.residents.everyone() {
        if !adopt_one(node, &resident, true).await {
            failed.push(resident.about.id.clone());
        }
    }
    if failed.is_empty() {
        return;
    }
    let node = Arc::downgrade(node);
    tokio::spawn(async move {
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
    state: std::sync::Mutex<Reports>,
    workers: crate::leases::Registry<String, fastctx_platform::doing::Mutex<()>>,
    starting: std::sync::Mutex<usize>,
}

/// Held while what a thread reported is applied (`drain`).
pub(crate) fn working(node: &Node, thread: &str) -> crate::leases::Lease<String, fastctx_platform::doing::Mutex<()>> {
    node.unclaimed.workers.get_or_insert_with(&thread.to_string(), || crate::ranked::working(thread))
}

/// Its conversation was deleted: what its thread reported and is still
/// kept goes with it.
pub(crate) fn deleted(node: &Node, thread: &str) {
    let mut state = node.unclaimed.state.lock().expect("never poisoned");
    state.queues.remove(thread);
    if state.queues.is_empty() {
        state.queues = HashMap::new();
    }
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
        // The reports' lock first, as keeping a report takes them.
        if none {
            let mut state = self.node.unclaimed.state.lock().expect("never poisoned");
            state.queues.retain(|thread, _| self.node.residents.resident_of_thread(thread).is_some());
            if *self.node.unclaimed.starting.lock().expect("never poisoned") == 0
                && let Err(error) = discard_unclaimed(&self.node)
            {
                self.node.log.line(format_args!("removing reports of threads nobody claimed: {error}"));
            }
        }
    }
}

/// Removes what was kept of threads nobody claimed. The caller holds the
/// reports' lock, so no report is being kept meanwhile; a claimed thread's
/// reports were taken in when it was claimed.
fn discard_unclaimed(node: &Node) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(node.world.path().join("codex-reports")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut unreadable = None;
    for entry in entries {
        let path = entry?.path();
        let claimed = if path.extension().is_some_and(|extension| extension == "json") {
            match serde_json::from_slice::<KeptReports>(&std::fs::read(&path)?) {
                Ok(kept) => node.residents.resident_of_thread(&kept.thread).is_some(),
                Err(error) => {
                    unreadable.get_or_insert_with(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("preserved {}: its reports cannot be read: {error}", path.display()),
                        )
                    });
                    continue;
                }
            }
        } else {
            false
        };
        if !claimed {
            std::fs::remove_file(path)?;
        }
    }
    match unreadable {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[derive(Default)]
struct Reports {
    queues: HashMap<String, Vec<HostEvent>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct KeptReports {
    thread: String,
    events: Vec<HostEvent>,
}

fn pending_path(node: &Node, thread: &str) -> std::path::PathBuf {
    match node.residents.resident_of_thread(thread) {
        Some(id) => node.world.conversation(&id).books().join("host-reports.jsonl"),
        None => node
            .world
            .path()
            .join("codex-reports")
            .join(format!("{}.jsonl", crate::speech::fixed_id("thread", &[thread]))),
    }
}

fn restore(node: &Node, thread: &str, state: &mut Reports) -> std::io::Result<()> {
    if !state.queues.contains_key(thread) {
        let path = pending_path(node, thread).with_extension("json");
        match std::fs::read(path) {
            Ok(bytes) => {
                let kept: KeptReports = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
                if !kept.events.is_empty() {
                    state.queues.insert(thread.to_string(), kept.events);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let legacy =
        node.world.path().join("codex-reports").join(format!("{}.json", crate::speech::fixed_id("thread", &[thread])));
    if legacy != pending_path(node, thread).with_extension("json") {
        match std::fs::read(&legacy) {
            Ok(bytes) => {
                let kept: KeptReports = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
                if kept.thread != thread {
                    return Err(std::io::Error::other(format!("{} belongs to another thread", legacy.display())));
                }
                let mut events = kept.events;
                let mut seen: std::collections::HashSet<_> =
                    events.iter().map(|event| serde_json::to_vec(event).expect("report serializes")).collect();
                for event in state.queues.get(thread).into_iter().flatten() {
                    if seen.insert(serde_json::to_vec(event).expect("report serializes")) {
                        events.push(event.clone());
                    }
                }
                save(node, thread, &events)?;
                if !events.is_empty() {
                    state.queues.insert(thread.to_string(), events);
                }
                std::fs::remove_file(&legacy)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let path = pending_path(node, thread);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut events = state.queues.get(thread).cloned().unwrap_or_default();
    let mut seen: std::collections::HashSet<_> =
        events.iter().map(|event| serde_json::to_vec(event).expect("report serializes")).collect();
    let mut incomplete = false;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let event: HostEvent = match serde_json::from_slice(line) {
            Ok(event) => event,
            Err(_) if line.last() != Some(&b'\n') => {
                incomplete = true;
                break;
            }
            Err(error) => return Err(std::io::Error::other(format!("{}: {error}", path.display()))),
        };
        if seen.insert(serde_json::to_vec(&event).expect("report serializes")) {
            events.push(event);
        }
    }
    save(node, thread, &events)?;
    if incomplete {
        let kept = path.with_extension(format!("{}.incomplete", crate::speech::new_id("partial")));
        fastctx_platform::publish_new(&kept, &bytes, fastctx_platform::Permissions::Private)?;
        std::fs::remove_file(&path)?;
        node.log.line(format_args!("an interrupted Codex report append left an incomplete final record; complete records recovered, original bytes retained at {}", kept.display()));
    } else {
        std::fs::remove_file(path)?;
    }
    if !events.is_empty() {
        state.queues.insert(thread.to_string(), events);
    }
    Ok(())
}

/// Whether a report is kept on disk until it is applied.
fn durable(event: &HostEvent) -> bool {
    match event {
        HostEvent::Synchronized { .. } => false,
        HostEvent::Happened { event, .. } => event.happened.kept(),
        HostEvent::Unloaded { .. } | HostEvent::Desk { .. } | HostEvent::Gone { .. } => true,
    }
}

fn save(node: &Node, thread: &str, events: &[HostEvent]) -> std::io::Result<()> {
    let path = pending_path(node, thread).with_extension("json");
    fastctx_platform::ensure_private_dir(path.parent().expect("report directory"))?;
    let events: Vec<_> = events.iter().filter(|event| durable(event)).cloned().collect();
    if events.is_empty() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
    }
    let kept = KeptReports { thread: thread.to_string(), events };
    fastctx_platform::publish(
        &path,
        &serde_json::to_vec(&kept).map_err(std::io::Error::other)?,
        fastctx_platform::Permissions::Private,
    )
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
    {
        let mut state = node.unclaimed.state.lock().expect("never poisoned");
        let claimed = node.residents.resident_of_thread(&thread).is_some();
        if !claimed {
            let starting = *node.unclaimed.starting.lock().expect("never poisoned") > 0;
            if starting && durable(&event) {
                state.queues.entry(thread).or_default().push(event.clone());
            }
            node.codex.acknowledge(&event);
            node.handoff.changed();
            return Ok(());
        }
        restore(node, &thread, &mut state)?;
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
    if node.residents.resident_of_thread(thread).is_none() {
        return;
    }
    let _doing = fastctx_platform::doing::doing(format!("reading the reports of Codex thread {thread}"));
    let worker = working(node, thread);
    let _worker = worker.lock().await;
    if node.residents.resident_of_thread(thread).is_none() {
        return;
    }
    if let Err(error) = restore(node, thread, &mut node.unclaimed.state.lock().expect("never poisoned")) {
        node.log.line(format_args!("restoring reports for {thread}: {error}"));
        return;
    }
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
            HostEvent::Synchronized { .. } => unreachable!("barriers do not enter the durable queue"),
            HostEvent::Unloaded { .. } => {
                if node
                    .residents
                    .about(&id)
                    .is_some_and(|(about, _)| !node.codex.of(node, &about).is_ok_and(|codex| codex.loaded(thread)))
                {
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
                    let facts =
                        facts.iter().find(|(t, id, _)| t == &thread && id == &turn).map(|(_, _, facts)| facts.clone());
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
        };
        if let Err(error) = result {
            node.log.line(format_args!("applying a retained report for {thread}: {error}"));
            return;
        }
        let mut state = node.unclaimed.state.lock().expect("never poisoned");
        // Its conversation deleted meanwhile, its reports went with it.
        let Some(events) = state.queues.get_mut(thread).filter(|events| !events.is_empty()) else { return };
        if durable(&events[0])
            && let Err(error) = save(node, thread, &events[1..])
        {
            node.log.line(format_args!("acknowledging a report for {thread}: {error}"));
            return;
        }
        let applied = events.remove(0);
        if events.is_empty() {
            state.queues.remove(thread);
            if state.queues.is_empty() {
                state.queues = HashMap::new();
            }
        }
        node.codex.acknowledge(&applied);
        if matches!(
            &applied,
            HostEvent::Happened {
                event: SessionEvent { happened: fastctx_types::session::Happened::TurnEnded { .. }, .. },
                ..
            } | HostEvent::Gone { .. }
        ) {
            node.watchers.changed.notify_one();
        }
        node.handoff.changed();
    }
}

/// Restores report queues before hearing any more from the new app-server.
/// Nothing is being started as the node starts: what was kept of a thread
/// nobody claimed goes.
pub fn listen(node: Arc<Node>, mut events: mpsc::UnboundedReceiver<HostEvent>) {
    {
        let mut state = node.unclaimed.state.lock().expect("never poisoned");

        for resident in node.residents.everyone() {
            if let Some(thread) = resident.about.session
                && let Err(error) = restore(&node, &thread, &mut state)
            {
                node.log.line(format_args!("restoring pending reports for {thread}: {error}"));
            }
        }
        if let Err(error) = discard_unclaimed(&node) {
            node.log.line(format_args!("removing reports of threads nobody claimed: {error}"));
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
                HostEvent::Synchronized { thread, done } => {
                    // In a task of its own: what goes wrong with one thread
                    // stays with it, and listening goes on.
                    let (draining, drained) = (node.clone(), thread.clone());
                    if let Err(error) = tokio::spawn(async move { drain(&draining, &drained).await }).await {
                        node.log.line(format_args!("applying what {thread} reported: {error}"));
                    }
                    let applied = if node
                        .unclaimed
                        .state
                        .lock()
                        .expect("never poisoned")
                        .queues
                        .get(&thread)
                        .is_none_or(Vec::is_empty)
                    {
                        Ok(())
                    } else {
                        Err("some Codex events could not be applied; see the node log".to_string())
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
