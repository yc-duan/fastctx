//! Reasons to keep a host runtime, and opportunistic retirement.

use super::{ReleaseReason, adapters::Adapter};
use crate::Node;
use fastctx_types::{ResidentId, ipc::Failure, residents::Activity, session::control::ControlState};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

const IDLE: Duration = Duration::from_secs(90 * 60);
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

pub(crate) fn held(node: &Node, id: &ResidentId) -> Result<bool, Failure> {
    let Some(now) = node.residents.now(id) else { return Ok(true) };
    if now.activity == Activity::Busy
        || now.turn.is_some()
        || now.waiting_for_user
        || now.asked.as_ref().is_some_and(|asked| asked.waits)
        || now.desk
        || node.desk.held_by(id)
        || node.watchers.watched(id)
        || node.attached.contains(id)
        || node.residents.is_deleting(id)
    {
        return Ok(true);
    }
    if crate::control_queue::pending(node, id)?
        .iter()
        .any(|row| matches!(row.state, ControlState::Waiting | ControlState::Running))
    {
        return Ok(true);
    }
    // Context that rides with the next turn does not itself require a runtime.
    // Untaken plugin envelopes return to Waiting when that activation ends.
    Ok(crate::inbox::pending(&node.world.conversation(id))
        .map_err(|error| Failure::new(error.to_string()))?
        .iter()
        .any(|row| row.delivery.notice.along().is_none()))
}

async fn consider(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    if !about.world_managed() || node.residents.removed(id).is_some() {
        return Ok(());
    }
    let Some(adapter) = Adapter::for_conversation(&about).filter(|adapter| !adapter.delayed()) else { return Ok(()) };
    if node.watchers.watched(id) && adapter.needs_subscription(node, &about)? {
        let actions = crate::control_queue::gate(node, id);
        let _action = actions.lock().await;
        let management = node.residents.management(id)?;
        let _reading = management.read().await;
        if node.watchers.watched(id) && node.residents.active(id).is_ok() {
            super::wake::wake(node, id, super::wake::Source::View, None).await.map_err(|error| error.failure)?;
        }
        return Ok(());
    }
    if held(node, id)? {
        return Ok(());
    }
    if adapter.candidate(node, &about).await?.is_none() {
        return Ok(());
    }
    let action = crate::control_queue::gate(node, id);
    let Ok(_action) = action.try_lock() else { return Ok(()) };
    let management = node.residents.management(id)?;
    let Ok(_exclusive) = management.try_write() else { return Ok(()) };
    if node.residents.removed(id).is_some() || held(node, id)? {
        return Ok(());
    }
    let Some((current, _)) = node.residents.about(id) else { return Ok(()) };
    if current.session != about.session || current.home != about.home {
        return Ok(());
    }
    let Some(candidate) = adapter.candidate(node, &current).await? else { return Ok(()) };
    if held(node, id)? {
        return Ok(());
    }
    super::release_locked(node, &current, ReleaseReason::Idle, Some(&candidate)).await?;
    Ok(())
}

pub(crate) fn start(node: &Arc<Node>) {
    super::sweep::start(node);
    let node = node.clone();
    tokio::spawn(async move {
        let running = Arc::new(Mutex::new(HashSet::new()));

        let mut timer = tokio::time::interval(SWEEP);
        loop {
            tokio::select! {
                _ = timer.tick() => {},
                () = node.watchers.changed.notified() => {},
            }
            // Coalesce reports before scanning. No gate or registry lock is held.
            tokio::time::sleep(Duration::from_millis(100)).await;
            for resident in node.residents.everyone() {
                if !running.lock().expect("never poisoned").insert(resident.id.clone()) {
                    continue;
                }
                let (node, running) = (node.clone(), running.clone());
                tokio::spawn(async move {
                    if let Err(error) = consider(&node, &resident.id).await {
                        node.log.line(format_args!("lifecycle {}: retaining host; {error}", resident.id));
                    }
                    running.lock().expect("never poisoned").remove(&resident.id);
                });
            }
        }
    });
}
