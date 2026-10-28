//! Stops are classified before their work is scheduled; live turns are never queried for quota.

use super::{checkpoint, clock, commit, failure, generation, materials, new_work, policy, timing};
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::goals::runtime::{QuotaSource, Task};
use fastctx_types::goals::{Goal, Status, View};
use fastctx_types::ipc::Failure;

use std::sync::Arc;

/// How long words judged not known to have gone in hold the goal back: as
/// long as their host may still say it took them. After that they are what
/// the user sees and decides on, and the goal goes on.
const UNSETTLED: std::time::Duration = std::time::Duration::from_secs(5 * 60);

pub(super) async fn quiet(node: &Arc<Node>, id: &ResidentId) -> Result<bool, Failure> {
    if crate::here::being_born(node, id)
        || crate::control_queue::busy(node, id)?
        || crate::composer::input_busy(node, id).await?
    {
        return Ok(false);
    }
    let directory = node.world.conversation(id);
    let (pending, _) = crate::inbox::input_states(&directory).map_err(failure)?;
    // Facts that ride along cannot start a turn or prevent the goal from starting one.
    Ok(pending.iter().all(|input| input.delivery.notice.along().is_some())
        && !crate::inbox::uncertain_lately(&directory, UNSETTLED).map_err(failure)?
        && crate::control_queue::pending(node, id)?.is_empty())
}

pub(super) fn assistance(node: &Node, goal: &Goal) -> Result<Option<policy::QuotaHold>, Failure> {
    let id = &goal.resident.resident;
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new("unknown conversation"))?;
    let selected = crate::session_options::read(node, id)?.model.or(about.model.clone());
    let Some(used) = crate::usage::used_up(node, &about, &goal.account, selected.as_deref()) else { return Ok(None) };
    Ok(used.expires().map(|reset| policy::QuotaHold {
        source: QuotaSource::Assistance,
        reset,
        reason: used.said(&goal.account),
    }))
}

pub(super) async fn current(node: &Arc<Node>, id: &ResidentId) -> Result<Option<View>, Failure> {
    let cached = node.goals.views.lock().expect("goal views").get(id).cloned();
    let Some(cached) = cached else { return Ok(None) };
    let address = fastctx_types::Address { machine: node.machine(), resident: id.clone() };
    let view = super::super::operations::read(node, &address, &cached.account).await?;
    Ok(Some(view))
}

pub(super) async fn reconcile(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let gate = crate::control_queue::gate(node, id);
    let _action = gate.lock().await;
    let _place = node.places.using(id.as_str()).await;
    if node.residents.is_deleting(id) || node.residents.removed(id).is_some() {
        return Ok(());
    }
    let Some(view) = node.goals.views.lock().expect("goal views").get(id).cloned() else {
        return Ok(());
    };
    if view.pending.is_some() {
        return Ok(());
    }
    let Some(mut goal) = view.goal.clone() else {
        return Ok(());
    };
    if goal.retired.is_some() || goal.status == Status::Complete {
        return Ok(());
    }
    if goal.active()
        && let Some((about, _)) = node.residents.about(id)
        && let Err(error) = crate::assists::check_turn(node, id, &about)
        && error.permanent
    {
        return commit(node, &view, policy::host_refused(goal, error.failure.message)).await;
    }
    timing(node, id, &view)?;
    let restarted = checkpoint::restore(node, &mut goal)?;
    if goal.runtime.pending.as_ref().is_some_and(|work| matches!(work.task, Task::Compact)) {
        return Ok(());
    }
    if crate::control_queue::busy(node, id)? {
        if goal.active()
            && node.turn_activity.get(id).is_some_and(|activity| activity.meaningful)
            && let Some(decision) = policy::healthy(&goal)
        {
            return commit(node, &view, decision).await;
        }
        return Ok(());
    }
    if !goal.active() && goal.claim.is_none() {
        return Ok(());
    }
    let latest = node.residents.now(id).and_then(|now| now.last_turn);
    let new_turn = latest.is_some() && latest != goal.runtime.settled_turn;
    let mut observed = if new_turn || !goal.runtime.waiting.is_empty() {
        Some(crate::turn_evidence::snapshot_locked(node, id, latest, false).await?)
    } else {
        None
    };
    if let Some(facts) = &observed
        && let Some(decision) = super::returned::decision(node, &goal, facts, new_turn)?
    {
        return commit(node, &view, decision).await;
    }
    if let Some(facts) = &observed
        && let Some(decision) = super::waiting::refresh(&goal, &facts.waiting)
    {
        return commit(node, &view, decision).await;
    }
    if new_turn {
        let mut facts = observed.take().expect("observed new turn");
        if goal.runtime.compacted_after.is_some()
            && facts
                .end
                .as_ref()
                .and_then(|end| end.facts.as_ref())
                .is_some_and(|facts| facts.world_compacted && !facts.world_progress)
        {
            goal.runtime.settled_turn = facts.turn.clone();
            return commit(
                node,
                &view,
                policy::Decision {
                    goal,
                    kind: "compaction_turn_settled",
                    reason: "The host's compaction turn ended.".into(),
                },
            )
            .await;
        }
        if let Some((key, _)) = materials::recent_owner(node, &goal)?.last()
            && let Some(decision) = policy::owner_spoke(&goal, key)
        {
            return commit(node, &view, decision).await;
        }

        let normal =
            facts.stop.as_ref().is_some_and(|stop| stop.kind == fastctx_types::turn_evidence::StopKind::Completed);
        if !normal || quiet(node, id).await? {
            if !facts.stop.as_ref().is_some_and(|stop| {
                matches!(
                    stop.kind,
                    fastctx_types::turn_evidence::StopKind::UserStopped
                        | fastctx_types::turn_evidence::StopKind::ContextWindowExceeded
                )
            }) {
                facts.quota = Some(crate::turn_evidence::quota::read(node, id).await);
            }
            if let Some(decision) =
                policy::ended(&goal, &facts, assistance(node, &goal)?, &clock(node), new_work(), generation(node, id))
            {
                if matches!(decision.kind, "idle_window" | "review_requested") {
                    waiting_known(&facts.waiting)?;
                }
                return commit(node, &view, decision).await;
            }
        }
    }
    if restarted && let Some(decision) = policy::retry_resumed(&goal, &clock(node), new_work()) {
        return commit(node, &view, decision).await;
    }
    if goal.runtime.pending.is_none() && quiet(node, id).await? {
        if let Some(decision) = policy::activated(&goal, &clock(node), new_work()) {
            return commit(node, &view, decision).await;
        }
        // A cancelled window may become idle again without another turn ending.
        if goal.active() && goal.runtime.settled_turn.is_some() {
            let facts = match observed.take() {
                Some(facts) => facts,
                None => {
                    crate::turn_evidence::snapshot_locked(node, id, goal.runtime.settled_turn.clone(), false).await?
                }
            };
            waiting_known(&facts.waiting)?;
            let handles = policy::waiting(&facts.waiting);
            goal.runtime.waiting = handles;
            let since = clock(node).now;
            let (task, seconds) = if goal.runtime.waiting.is_empty() {
                (Task::Idle { generation: generation(node, id) }, policy::IDLE_SECONDS)
            } else {
                (Task::Remind { since }, policy::REMINDER_SECONDS)
            };
            goal.runtime.pending = Some(fastctx_types::goals::runtime::Work {
                id: new_work(),
                due: since.checked_add(std::time::Duration::from_secs(seconds)).map_err(failure)?,
                turn: goal.runtime.settled_turn.clone(),
                task,
            });
            return commit(
                node,
                &view,
                policy::Decision {
                    goal,
                    kind: "idle_observation",
                    reason: "Observing the stopped conversation again.".into(),
                },
            )
            .await;
        }
    }
    Ok(())
}

pub(super) fn waiting_known(handles: &[fastctx_types::turn_evidence::WaitHandle]) -> Result<(), Failure> {
    if !policy::waiting(handles).is_empty() {
        return Ok(());
    }
    if let Some(handle) = handles.iter().find(|handle| {
        !handle.will_wake && matches!(handle.liveness, fastctx_types::turn_evidence::Liveness::Unknown { .. })
    }) {
        return Err(Failure::new(format!(
            "cannot establish whether {} on {} will wake the conversation: {}",
            handle.id, handle.machine, handle.summary
        )));
    }
    Ok(())
}
