//! A small handoff queue separates host callbacks from persistence. Durable
//! bodies leave memory after insertion; live-only reports retain their order
//! relative to the latest committed durable position without being persisted.

use super::{HostEvent, Node, report_store};
use std::{
    collections::{HashMap, VecDeque},
    io,
    sync::Arc,
};

#[derive(Default)]
pub(super) struct State {
    pub queues: HashMap<String, Queue>,
}
#[derive(Default)]
pub(super) struct Queue {
    incoming: VecDeque<(String, HostEvent)>,
    live: VecDeque<(u64, String, HostEvent)>,
    tail: Option<u64>,
    applied: Option<Selected>,
    scheduled: bool,
    flushing: bool,
}
#[derive(Clone)]
pub(super) enum Identity {
    Stored(u64),
    Live(String),
}
#[derive(Clone)]
pub(super) struct Selected {
    identity: Identity,
    pub event: HostEvent,
    pub applied: bool,
}
pub(super) enum Next {
    Event(Box<Selected>),
    More,
    Empty,
}

pub(super) fn push(node: &Arc<Node>, thread: String, event: HostEvent) -> (bool, bool) {
    let mut state = node.unclaimed.state.lock().expect("host reports");
    let claimed = node.residents.resident_of_thread(&thread).is_some();
    if !claimed {
        if *node.unclaimed.starting.lock().expect("host starting") > 0 && super::durable(&event) {
            state
                .queues
                .entry(thread)
                .or_default()
                .incoming
                .push_back((crate::speech::new_id("host-report").to_string(), event.clone()));
        }
        node.codex.acknowledge(&event);
        node.handoff.changed();
        return (false, false);
    }
    let queue = state.queues.entry(thread).or_default();
    queue.incoming.push_back((crate::speech::new_id("host-report").to_string(), event));
    let start = !queue.scheduled;
    let flush = !queue.flushing;
    queue.flushing = true;
    queue.scheduled = true;
    (flush, start)
}
pub(super) async fn persist(node: Arc<Node>, thread: String) {
    loop {
        if node.residents.resident_of_thread(&thread).is_none() {
            if let Some(queue) = node.unclaimed.state.lock().expect("host reports").queues.get_mut(&thread) {
                queue.flushing = false;
            }
            return;
        }
        let result = flush(&node, &thread).await;
        let done = {
            let mut state = node.unclaimed.state.lock().expect("host reports");
            let Some(queue) = state.queues.get_mut(&thread) else { return };
            let done = result.is_err() || queue.incoming.is_empty();
            if done {
                queue.flushing = false;
            }
            done
        };
        if let Err(error) = result {
            node.log.line(format_args!("retaining host reports for {thread}: {error}"));
            return;
        }
        if done {
            return;
        }
        tokio::task::yield_now().await;
    }
}
fn persisting(node: &Node, thread: &str) -> crate::leases::Lease<String, tokio::sync::Mutex<()>> {
    node.unclaimed.persisting.get_or_insert_with(&thread.to_string(), || tokio::sync::Mutex::new(()))
}
async fn local<T: Send + 'static>(action: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    fastctx_platform::task::spawn_blocking(action).await.map_err(io::Error::other)?
}
/// Flush only the finite prefix present on entry, so a continuous host cannot
/// keep an already committed report waiting for the incoming stream to stop.
pub(super) async fn flush(node: &Arc<Node>, thread: &str) -> io::Result<()> {
    if node.residents.resident_of_thread(thread).is_none() {
        return Ok(());
    }
    let gate = persisting(node, thread);
    let _persisting = gate.lock().await;
    let (initialize, fence) = {
        let mut state = node.unclaimed.state.lock().expect("host reports");
        let queue = state.queues.entry(thread.to_string()).or_default();
        (queue.tail.is_none(), queue.incoming.back().map(|(id, _)| id.clone()))
    };
    if initialize {
        let (reading, name) = (node.clone(), thread.to_string());
        let tail = local(move || report_store::latest(&reading, &name)).await?;
        let mut state = node.unclaimed.state.lock().expect("host reports");
        let Some(queue) = state.queues.get_mut(thread) else { return Ok(()) };
        queue.tail = Some(tail);
    }
    let Some(fence) = fence else { return Ok(()) };
    loop {
        let pending = node
            .unclaimed
            .state
            .lock()
            .expect("host reports")
            .queues
            .get(thread)
            .and_then(|queue| queue.incoming.front())
            .cloned();
        let Some((id, event)) = pending else { return Ok(()) };
        let position = if super::durable(&event) {
            let (writing, name, key, kept) = (node.clone(), thread.to_string(), id.clone(), event.clone());
            Some(local(move || report_store::append(&writing, &name, key, &kept)).await?)
        } else {
            None
        };
        let mut state = node.unclaimed.state.lock().expect("host reports");
        let Some(queue) = state.queues.get_mut(thread) else { return Ok(()) };
        if queue.incoming.front().is_none_or(|(key, _)| *key != id) {
            return Err(io::Error::other("host handoff order changed"));
        }
        if let Some(position) = position {
            queue.tail = Some(position);
        } else {
            queue.live.push_back((queue.tail.expect("initialized queue"), id.clone(), event));
        }
        queue.incoming.pop_front();
        if id == fence {
            return Ok(());
        }
    }
}
pub(super) async fn next(node: &Arc<Node>, thread: &str) -> io::Result<Next> {
    let gate = persisting(node, thread);
    let _persisting = gate.lock().await;
    if let Some(applied) =
        node.unclaimed.state.lock().expect("host reports").queues.get(thread).and_then(|queue| queue.applied.clone())
    {
        return Ok(Next::Event(Box::new(applied)));
    }
    let (reading, name) = (node.clone(), thread.to_string());
    let stored = local(move || report_store::first(&reading, &name)).await?;
    let mut state = node.unclaimed.state.lock().expect("host reports");
    let queue = state.queues.entry(thread.to_string()).or_default();
    if let Some((after, id, event)) = queue.live.front()
        && stored.as_ref().is_none_or(|(position, _)| position > after)
    {
        return Ok(Next::Event(Box::new(Selected {
            identity: Identity::Live(id.clone()),
            event: event.clone(),
            applied: false,
        })));
    }
    if let Some((position, event)) = stored {
        return Ok(Next::Event(Box::new(Selected { identity: Identity::Stored(position), event, applied: false })));
    }
    if !queue.incoming.is_empty() {
        return Ok(Next::More);
    }
    queue.scheduled = false;
    Ok(Next::Empty)
}
pub(super) fn applied(node: &Node, thread: &str, selected: &Selected) {
    if let Some(queue) = node.unclaimed.state.lock().expect("host reports").queues.get_mut(thread) {
        let mut selected = selected.clone();
        selected.applied = true;
        queue.applied = Some(selected);
    }
}
pub(super) async fn acknowledge(node: &Arc<Node>, thread: &str, selected: &Selected) -> io::Result<()> {
    if let Identity::Stored(position) = selected.identity {
        let (writing, name) = (node.clone(), thread.to_string());
        local(move || report_store::acknowledge(&writing, &name, position)).await?;
    }
    let mut state = node.unclaimed.state.lock().expect("host reports");
    if let Some(queue) = state.queues.get_mut(thread) {
        if let Identity::Live(ref id) = selected.identity {
            if queue.live.front().is_none_or(|(_, key, _)| key != id) {
                return Err(io::Error::other("live host report order changed"));
            }
            queue.live.pop_front();
        }
        queue.applied = None;
    }
    Ok(())
}
pub(super) fn failed(node: &Node, thread: &str) {
    if let Some(queue) = node.unclaimed.state.lock().expect("host reports").queues.get_mut(thread) {
        queue.scheduled = false;
    }
}
pub(super) async fn empty(node: &Arc<Node>, thread: &str) -> io::Result<bool> {
    let memory = node
        .unclaimed
        .state
        .lock()
        .expect("host reports")
        .queues
        .get(thread)
        .is_none_or(|queue| queue.incoming.is_empty() && queue.live.is_empty() && queue.applied.is_none());
    if !memory {
        return Ok(false);
    }
    if node.residents.resident_of_thread(thread).is_none() {
        return Ok(true);
    }
    let (reading, name) = (node.clone(), thread.to_string());
    local(move || report_store::first(&reading, &name).map(|row| row.is_none())).await
}

pub(super) fn pending(node: &Node, thread: &str) -> bool {
    node.unclaimed
        .state
        .lock()
        .expect("host reports")
        .queues
        .get(thread)
        .is_some_and(|queue| !queue.incoming.is_empty() || !queue.live.is_empty() || queue.applied.is_some())
}
