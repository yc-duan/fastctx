//! Reasons to keep a host runtime, and opportunistic retirement.
//!
//! A conversation's host runtime is kept for the model's activity alone
//! (2026-11-16): a turn running, and, for Claude, for `IDLE` after the model
//! last acted (`sweep`). Who looks at the conversation — a window that keeps
//! it open, a subscription, `fastctx attach` — and a card that waits for the
//! user's answer keep nothing: what wakes a conversation (its words, the
//! World's events) wakes it again. The few seconds a word is handed over, a
//! control runs or the conversation is deleted are not a reason to keep it
//! but steps not to be cut through.

use super::{ReleaseReason, adapters::Adapter};
use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure, residents::Activity, session::control::ControlState};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const IDLE: Duration = Duration::from_secs(75 * 60);
const SWEEP: Duration = Duration::from_secs(60);

pub(super) fn idle_delay() -> Duration {
    if !fastctx_platform::home::RELEASED
        && let Ok(value) = std::env::var("FASTCTX_LIFECYCLE_IDLE_SECONDS")
        && let Ok(seconds) = value.parse::<u64>()
        && seconds > 0
    {
        return Duration::from_secs(seconds);
    }
    IDLE
}

/// Whether the model last acted `idle_delay` ago or longer: as the World
/// saw its turns begin and end (`now.since`), and as the host last wrote the
/// work of its own it runs between them (`host`, a Claude worker's job).
pub(super) fn rested(since: jiff::Timestamp, host: Option<jiff::Timestamp>) -> bool {
    let acted = host.map_or(since, |host| host.max(since));
    jiff::Timestamp::now().as_second().saturating_sub(acted.as_second()) >= idle_delay().as_secs() as i64
}

pub(crate) async fn held(node: &Node, id: &ResidentId) -> Result<bool, Failure> {
    let Some(now) = node.residents.now(id) else { return Ok(true) };
    // The model at work, a turn running; and a deletion, not cut through.
    if now.activity == Activity::Busy || now.turn.is_some() || node.residents.is_deleting(id) {
        return Ok(true);
    }
    if crate::control_queue::pending(node, id)
        .await?
        .iter()
        .any(|row| matches!(row.state, ControlState::Waiting | ControlState::Running))
    {
        return Ok(true);
    }
    // Words on their way into the host, handed over or waking it. Context
    // that rides with the next turn does not itself require a runtime, nor
    // do words still waiting for a way in — a login's turn, say: they wake
    // the host when they go in (codex-4bf06087 #2). Untaken plugin envelopes
    // return to Waiting when that activation ends.
    let directory = node.world.conversation(id);
    fastctx_platform::task::spawn_blocking(move || {
        crate::inbox::any(&directory, |row| {
            row.delivery.notice.along().is_none() && row.state != fastctx_types::residents::PendingState::Waiting
        })
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(|error| Failure::new(error.to_string()))
}

async fn consider(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    if !about.world_managed() || node.residents.removed(id).is_some() {
        return Ok(());
    }
    let Some(adapter) = Adapter::for_conversation(&about).filter(|adapter| !adapter.delayed()) else { return Ok(()) };
    // A Codex thread is loaded for a turn and let go as it ends: one looked
    // at is shown from what the World keeps of it, never loaded to be seen.
    if held(node, id).await? {
        return Ok(());
    }
    if adapter.candidate(node, &about).await?.is_none() {
        return Ok(());
    }
    let action = crate::control_queue::gate(node, id);
    let Ok(_action) = action.try_lock() else { return Ok(()) };
    let management = node.residents.management(id)?;
    let Ok(_exclusive) = management.try_write() else { return Ok(()) };
    if node.residents.removed(id).is_some() || held(node, id).await? {
        return Ok(());
    }
    let Some((current, _)) = node.residents.about(id) else { return Ok(()) };
    if current.session != about.session || current.home != about.home {
        return Ok(());
    }
    let Some(candidate) = adapter.candidate(node, &current).await? else { return Ok(()) };
    if held(node, id).await? {
        return Ok(());
    }
    super::release_locked(node, &current, ReleaseReason::Idle, Some(&candidate)).await?;
    Ok(())
}

pub(crate) fn start(node: &Arc<Node>) {
    super::sweep::start(node);
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        // Those being looked at, each with whether it was told of again
        // meanwhile: it is looked at once more as that ends.
        let running = Arc::new(Mutex::new(HashMap::<ResidentId, bool>::new()));

        let mut timer = tokio::time::interval(SWEEP);
        loop {
            // Each round looks at everyone; between rounds, at those whose
            // watching or turns changed.
            let everyone = tokio::select! {
                _ = timer.tick() => true,
                () = node.watchers.changed.told() => false,
            };
            // Coalesce reports before scanning. No gate or registry lock is held.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let told = node.watchers.changed.take();
            let ids: Vec<ResidentId> = if everyone {
                node.residents.everyone().map(|resident| resident.id).collect()
            } else {
                told.into_iter().collect()
            };
            for id in ids {
                {
                    let mut running = running.lock().expect("never poisoned");
                    if let Some(again) = running.get_mut(&id) {
                        *again = true;
                        continue;
                    }
                    running.insert(id.clone(), false);
                }
                let (node, running) = (node.clone(), running.clone());
                fastctx_platform::task::spawn(async move {
                    if let Err(error) = consider(&node, &id).await {
                        node.log.line(format_args!("lifecycle {id}: retaining host; {error}"));
                    }
                    if running.lock().expect("never poisoned").remove(&id).unwrap_or(false) {
                        node.watchers.changed.tell(&id);
                    }
                });
            }
        }
    });
}
