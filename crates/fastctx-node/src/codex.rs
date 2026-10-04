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
use fastctx_types::session::SessionEvent;
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

/// What the host reported of threads no resident is yet: Codex may report
/// on a thread before it answers the request that started it, and these
/// wait for the resident the thread becomes (`claim`).
#[derive(Default)]
pub struct Unclaimed(tokio::sync::Mutex<HashMap<String, Vec<SessionEvent>>>);

/// Hands what the host reports to the residents, in the order it comes.
pub fn listen(node: Arc<Node>, mut events: mpsc::UnboundedReceiver<HostEvent>) {
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                HostEvent::Happened { thread, event } => {
                    let mut unclaimed = node.unclaimed.0.lock().await;
                    match node.residents.resident_of_thread(&thread) {
                        Some(id) => {
                            drop(unclaimed);
                            crate::turns::happened(&node, &id, event).await;
                        }
                        None => unclaimed.entry(thread).or_default().push(event),
                    }
                }
                HostEvent::Desk { thread, turn, desk } => {
                    if let Some(id) = node.residents.resident_of_thread(&thread) {
                        crate::desk::used(&node, &id, &turn, desk);
                    }
                }
                HostEvent::Gone { how, turns } => {
                    for (thread, turn) in turns {
                        if let Some(id) = node.residents.resident_of_thread(&thread) {
                            crate::turns::stopped(&node, &id, &turn, format!("its app-server stopped ({how})")).await;
                        }
                    }
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
pub async fn claim(node: &Arc<Node>, thread: &str, id: &ResidentId) {
    let mut unclaimed = node.unclaimed.0.lock().await;
    for event in unclaimed.remove(thread).unwrap_or_default() {
        crate::turns::happened(node, id, event).await;
    }
}
