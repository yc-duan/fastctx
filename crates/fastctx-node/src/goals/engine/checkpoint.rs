//! Local online-time checkpoints do not create account revisions or decision-history entries.

use super::{clock, failure, policy};
use crate::Node;
use fastctx_types::goals::Goal;
use fastctx_types::goals::runtime::Retry;
use fastctx_types::ipc::Failure;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Checkpoint {
    goal: String,
    source: Retry,
    progress: Retry,
    restarted: bool,
}

/// The conversation's action gate serializes this with decisions that incorporate its progress.
pub(super) fn restore(node: &Node, goal: &mut Goal) -> Result<bool, Failure> {
    if !goal.active() {
        return Ok(false);
    }
    let Some(source) = goal.runtime.retry.clone() else { return Ok(false) };
    let id = &goal.resident.resident;
    let path = node.world.conversation(id).books().join("goal-retry.json");
    let cached = node.goals.engine.checkpoints.lock().expect("goal checkpoints").get(id).cloned();
    let row = match cached {
        Some(row) => Some(row),
        None => match std::fs::read(&path) {
            Ok(bytes) => Some(serde_json::from_slice::<Checkpoint>(&bytes).map_err(failure)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(failure(error)),
        },
    };
    let mut row = row.filter(|row| row.goal == goal.id && row.source == source).unwrap_or_else(|| Checkpoint {
        goal: goal.id.clone(),
        progress: source.clone(),
        source,
        restarted: false,
    });
    let clock = clock(node);
    if row.progress.epoch != clock.epoch || clock.online_ms.saturating_sub(row.progress.checkpoint_ms) >= 30_000 {
        row.restarted |= policy::account_time(&mut row.progress, &clock);
        fastctx_platform::ensure_private_dir(path.parent().expect("books")).map_err(failure)?;
        fastctx_platform::publish(
            &path,
            &serde_json::to_vec(&row).map_err(failure)?,
            fastctx_platform::Permissions::Private,
        )
        .map_err(failure)?;
    }
    // Decisions use the current reading even when the next durable checkpoint is not due.
    let mut current = row.progress.clone();
    policy::account_time(&mut current, &clock);
    goal.runtime.retry = Some(current);
    let restarted = row.restarted;
    node.goals.engine.checkpoints.lock().expect("goal checkpoints").insert(id.clone(), row);
    Ok(restarted)
}
