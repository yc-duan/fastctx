//! The goal owns an autonomous completion promise, even between turns or while its report is in transit.

use super::{operations, scope};
use crate::Node;
use fastctx_types::layout::Here;
use fastctx_types::{
    Address, NoticeId, ResidentId,
    access::Acting,
    goals::{Goal, Owner, ReportDelivery, Status, runtime::Task},
    ipc::Failure,
    residents::{Notice, Origin},
    session::{Happened, SessionEvent},
    turn_evidence::{GoalDebt, Liveness, WaitHandle, WaitKind},
};
use std::sync::Arc;

pub(crate) fn continues(goal: &Goal) -> bool {
    goal.retired.is_none()
        && (goal.active()
            || goal.claim.is_some()
            || (goal.status == Status::Paused
                && goal.stop.as_ref().is_some_and(|stop| stop.resume_at.is_some())
                && goal.runtime.pending.as_ref().is_some_and(|work| matches!(work.task, Task::Quota { .. }))))
}

/// New runs use their accepted operation; older records retain their existing activation identity.
pub(crate) fn completion(goal: &Goal) -> NoticeId {
    let legacy;
    let activation = match goal.runtime.activation.as_deref() {
        Some(operation) => operation,
        None => {
            let at = goal.runtime.activated_at.unwrap_or(goal.created_at);
            legacy = format!("legacy:{at}");
            &legacy
        }
    };
    crate::speech::fixed_id(
        "goal-completion",
        &[goal.account.hub.as_str(), goal.account.account.as_str(), &goal.id, activation],
    )
}

pub(crate) async fn debt(
    node: &Arc<Node>,
    acting: &Acting,
    resident: &ResidentId,
    requester: &Address,
) -> Result<Option<GoalDebt>, Failure> {
    let Some((about, _)) = node.residents.about(resident) else { return Ok(None) };
    if !matches!(&about.origin, Origin::Colleague { asked_by, .. } if super::same(node, asked_by, requester)) {
        return Ok(None);
    }
    let Some(account) = scope::pinned(node, resident)? else { return Ok(None) };
    if !node.serves(acting, &account.hub, &account.account) {
        return Ok(None);
    }
    let view =
        operations::read(node, &Address { machine: node.machine(), resident: resident.clone() }, &account).await?;
    let Some(goal) = view.goal.as_ref().filter(|goal| goal.retired.is_none()) else { return Ok(None) };
    if !matches!(&goal.owner, Owner::Conversation { address } if super::same(node, address, requester)) {
        return Ok(None);
    }
    let continuing = view.pending.is_some() || continues(goal);
    let report = (!continuing)
        .then(|| {
            goal.runtime
                .conclusion
                .as_ref()
                .map(|conclusion| crate::speech::fixed_id("goal-result", &[&goal.id, &conclusion.operation]))
        })
        .flatten();
    Ok(Some(GoalDebt {
        account,
        completion: completion(goal),
        since: goal.runtime.activated_at.unwrap_or(goal.created_at),
        continuing,
        report,
        summary: super::words::goal_waiting(goal, view.pending.is_some()),
    }))
}

/// Metadata already in this requester's custody, including reports not yet consumed by its model.
pub(crate) fn received(node: &Node, resident: &ResidentId) -> Result<Vec<ReportDelivery>, Failure> {
    let directory = node.world.conversation(resident);
    let mut received = Vec::new();
    // Read custody in its forward order: pending input is published into history before it is removed.
    for pending in crate::inbox::pending(&directory).map_err(fail)? {
        if let Notice::Words { goal_report: Some(report), .. } = pending.delivery.notice {
            received.push(*report);
        }
    }
    for event in crate::delivery::history(&directory).map_err(fail)? {
        if let Happened::Heard { notice: Some(notice), .. } = event.happened
            && let Notice::Words { goal_report: Some(report), .. } = *notice
        {
            received.push(*report);
        }
    }
    Ok(received)
}

pub(crate) fn handle(
    node: &Node,
    colleague: &Address,
    debt: &GoalDebt,
    received: &[ReportDelivery],
) -> Option<WaitHandle> {
    if !debt.continuing && debt.report.is_none() {
        return None;
    }
    let arrived = !debt.continuing
        && debt.report.as_ref().is_some_and(|expected| {
            received.iter().any(|report| {
                report.id == *expected
                    && report.account == debt.account
                    && super::same(node, &report.colleague, colleague)
            })
        });
    Some(WaitHandle {
        id: colleague.resident.to_string(),
        kind: WaitKind::Colleague,
        machine: colleague.machine.clone(),
        since: Some(debt.since),
        liveness: if arrived { Liveness::Ended } else { Liveness::Alive },
        will_wake: !arrived,
        completion: Some(debt.completion.clone()),
        summary: if debt.continuing {
            debt.summary.clone()
        } else {
            super::words::goal_report_wait(&debt.summary, arrived)
        },
        output: None,
        output_growth: None,
    })
}

/// A known newer or still-outstanding goal observation takes precedence over an older report.
/// Unknown producers can still fulfill the exact promise carried in an authenticated report.
pub(crate) fn returned(
    node: &Node,
    waiting: &fastctx_types::goals::runtime::WaitKey,
    event: &SessionEvent,
    current: &[WaitHandle],
) -> bool {
    let Happened::Heard { notice: Some(notice), .. } = &event.happened else { return false };
    let Notice::Words { goal_report: Some(report), .. } = notice.as_ref() else { return false };
    if waiting.completion.is_none() || report.completion != waiting.completion {
        return false;
    }
    if report.colleague.resident.as_str() != waiting.id
        || !(report.colleague.machine == waiting.machine
            || node.names().names_of(&report.colleague.machine).contains(&waiting.machine))
    {
        return false;
    }
    !current.iter().any(|handle| {
        handle.kind == WaitKind::Colleague
            && handle.id == waiting.id
            && (handle.machine == waiting.machine || node.names().names_of(&handle.machine).contains(&waiting.machine))
            && handle.completion.is_some()
            && (handle.completion != waiting.completion || !matches!(handle.liveness, Liveness::Ended))
    })
}

fn fail(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
