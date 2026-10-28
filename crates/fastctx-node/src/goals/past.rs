//! A conversation's earlier goals (`world-goal.md`, 目标的历史): one done,
//! or put down when the user cleared it or set another, is kept as a line in
//! the `goal_past` account record beside the current goal's, oldest first and
//! the latest few. Written before the goal it follows is replaced, keyed by
//! the goal's id, so writing it again changes nothing; a reader leaves out the
//! one that is current.

use std::sync::Arc;

use fastctx_types::Address;
use fastctx_types::access::HubAccount;
use fastctx_types::goals::{Goal, GoalPast, PastGoal, PastOutcome, Status};
use fastctx_types::ipc::Failure;
use jiff::Timestamp;

use super::operations::bounded;
use super::records::Exchanged;
use crate::Node;

pub(crate) const KIND: &str = "goal_past";
/// The most earlier goals kept.
const KEPT: usize = 64;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// The earlier goals of the conversation at `resident`.
pub(crate) async fn read(node: &Arc<Node>, resident: &Address, account: &HubAccount) -> Result<GoalPast, Failure> {
    let record = bounded(node.goals.records.read(account, KIND, resident.resident.as_str())).await?;
    let past: GoalPast =
        record.and_then(|record| record.body).and_then(|body| serde_json::from_value(body).ok()).unwrap_or_default();
    Ok(if past.resident.as_ref().is_none_or(|owner| owner == resident) { past } else { GoalPast::default() })
}

/// Forgets the earlier goals of a conversation being deleted; called again
/// after an interruption, it finds nothing left to do.
pub(crate) async fn forget(node: &Arc<Node>, account: &HubAccount, resident: &Address) -> Result<(), Failure> {
    let key = resident.resident.as_str();
    for _ in 0..3 {
        let Some(record) = bounded(node.goals.records.read(account, KIND, key)).await? else { return Ok(()) };
        let Some(body) = record.body else { return Ok(()) };
        let past: GoalPast = serde_json::from_value(body).unwrap_or_default();
        if past.resident.as_ref().is_some_and(|owner| owner != resident) {
            return Ok(());
        }
        match bounded(node.goals.records.exchange(account, KIND, key, Some(record.version), None)).await? {
            Exchanged::Written { .. } => return Ok(()),
            Exchanged::Conflict { .. } => continue,
        }
    }
    Err(Failure::new("the earlier goals changed while they were deleted; try again"))
}

/// Keeps `goal` among the earlier goals, as it stood when it was replaced
/// or cleared at `at`.
pub(crate) async fn keep(node: &Arc<Node>, goal: &Goal, at: Timestamp) -> Result<(), Failure> {
    let line = PastGoal {
        id: goal.id.clone(),
        objective: goal.objective.clone(),
        outcome: if goal.status == Status::Complete { PastOutcome::Complete } else { PastOutcome::Dropped },
        created_at: goal.created_at,
        closed_at: at,
        statement: goal.statement.clone(),
        standing: goal.standing,
        parent: goal.parent.clone(),
    };
    let key = goal.resident.resident.as_str();
    for _ in 0..3 {
        let record = bounded(node.goals.records.read(&goal.account, KIND, key)).await?;
        let version = record.as_ref().map(|record| record.version);
        let mut past: GoalPast = record
            .and_then(|record| record.body)
            .and_then(|body| serde_json::from_value(body).ok())
            .unwrap_or_default();
        if past.goals.iter().any(|kept| kept.id == line.id) {
            return Ok(());
        }
        // Another machine's conversation of the same short name had them:
        // this conversation's begin anew.
        if past.resident.as_ref().is_some_and(|owner| owner != &goal.resident) {
            past.goals.clear();
        }
        past.resident = Some(goal.resident.clone());
        past.goals.push(line.clone());
        let excess = past.goals.len().saturating_sub(KEPT);
        past.goals.drain(..excess);
        let body = serde_json::to_value(&past).map_err(failure)?;
        match bounded(node.goals.records.exchange(&goal.account, KIND, key, version, Some(body))).await? {
            Exchanged::Written { .. } => return Ok(()),
            Exchanged::Conflict { .. } => continue,
        }
    }
    Err(Failure::new("the earlier goals changed while this one was kept; try again"))
}
