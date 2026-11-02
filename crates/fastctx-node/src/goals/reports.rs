//! Accepted conclusions are immutable facts, queued before an operation releases its proposed state.

pub(crate) use super::report_store::recent;
use super::report_store::{self, Frozen};
use crate::Node;
use fastctx_types::goals::{Goal, Owner, Report, Status, StopKind};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Delivery, Notice, Origin, Owing};
use std::sync::Arc;

fn body(goal: &Goal) -> Option<&str> {
    match goal.status {
        Status::Active => None,
        Status::Complete => goal.statement.as_deref(),
        Status::Blocked
            if goal.stop.as_ref().is_some_and(|stop| matches!(stop.kind, StopKind::Claimed | StopKind::Review)) =>
        {
            goal.statement.as_deref()
        }
        Status::Blocked | Status::Paused => goal.stop.as_ref().map(|stop| stop.reason.as_str()),
    }
}

fn fingerprint(goal: &Goal) -> String {
    let value = (&goal.status, body(goal), goal.stop.as_ref().and_then(|stop| stop.resume_at));
    fastctx_hands::files::content_hash(&serde_json::to_vec(&value).expect("conclusion fields serialize"))
}

/// Stamp a proposed conclusion; no notification exists until its account CAS is accepted.
pub(super) fn remember(goal: &mut Goal, operation: &str, at: jiff::Timestamp) {
    if goal.active()
        || goal.claim.is_some()
        || goal.retired.is_some()
        || !matches!(goal.owner, Owner::Conversation { .. })
    {
        goal.runtime.conclusion = None;
        return;
    }
    let fingerprint = fingerprint(goal);
    if goal.runtime.conclusion.as_ref().is_none_or(|prior| prior.fingerprint != fingerprint) {
        goal.runtime.conclusion =
            Some(fastctx_types::goals::runtime::Conclusion { operation: operation.into(), at, fingerprint });
    }
}

/// Called for both accepted intents and authoritative local views. A later edit or resume
/// cannot replace the report owed by an earlier accepted intent.
fn keep_now(node: &Arc<Node>, goal: &Goal, runtime: tokio::runtime::Handle) -> Result<(), Failure> {
    if !node.is_me(&goal.resident.machine) || node.residents.is_deleting(&goal.resident.resident) {
        return Ok(());
    }
    let Owner::Conversation { address: to } = &goal.owner else { return Ok(()) };
    let Some(conclusion) = &goal.runtime.conclusion else { return Ok(()) };
    if goal.active() || goal.claim.is_some() || goal.retired.is_some() {
        return Ok(());
    }
    if conclusion.fingerprint != fingerprint(goal) {
        return Err(Failure::new(super::words::REPORT_CHANGED));
    }
    let text = body(goal).ok_or_else(|| Failure::new(super::words::REPORT_MISSING))?;
    let id = crate::speech::fixed_id("goal-result", &[&goal.id, &conclusion.operation]);
    let conversation = node.world.conversation(&goal.resident.resident);

    let label = node
        .residents
        .about(&goal.resident.resident)
        .and_then(|(about, _)| match about.origin {
            Origin::Colleague { label, .. } => Some(label),
            Origin::User => None,
        })
        .unwrap_or_default();
    let proposed = Frozen {
        to: to.clone(),
        report: Report {
            id: id.clone(),
            goal: goal.id.clone(),
            colleague: goal.resident.clone(),
            account: goal.account.clone(),
            label,
            status: goal.status,
            started_at: goal.created_at,
            ended_at: conclusion.at,
            resume_at: goal.stop.as_ref().and_then(|stop| stop.resume_at),
            text: text.into(),
        },
    };
    report_store::prepare(&conversation).map_err(failure)?;
    crate::inbox::storage::initialize(&conversation).map_err(failure)?;
    let bytes = serde_json::to_vec(&proposed).map_err(failure)?.len();
    let (saving, goal, notify, recipient) = (node.clone(), goal.clone(), node.clone(), goal.resident.resident.clone());
    let paying = conversation.clone();
    crate::records::write_then_at(
        &conversation,
        fastctx_storage::Priority::Normal,
        bytes.saturating_mul(3),
        move |rows| {
            let frozen = report_store::keep_in(rows, proposed)?;
            crate::inbox::owe_in(rows, &owing(&saving, &goal, frozen))
        },
        move |_| {
            fastctx_platform::task::spawn_on(&runtime, async move {
                crate::turns::pay(&notify, &recipient, &paying).await;
            });
        },
    )
    .map_err(failure)
}

fn owing(node: &Node, goal: &Goal, frozen: Frozen) -> Owing {
    let id = frozen.report.id.clone();
    Owing {
        to: frozen.to,
        delivery: Delivery {
            id,
            at: frozen.report.ended_at,
            notice: Notice::Words {
                from: fastctx_types::session::Speaker::Resident { address: frozen.report.colleague.clone() },
                text: fastctx_hands::told::goal_report(&frozen.report, Some(&node.machine())),
                goal_question: None,
                goal_report: Some(Box::new(fastctx_types::goals::ReportDelivery {
                    id: frozen.report.id,
                    completion: (!super::waiting::continues(goal)).then(|| super::waiting::completion(goal)),
                    shown: Some(fastctx_types::goals::ReportShown {
                        label: frozen.report.label,
                        status: frozen.report.status,
                        started_at: frozen.report.started_at,
                        ended_at: frozen.report.ended_at,
                        resume_at: frozen.report.resume_at,
                        text: frozen.report.text,
                    }),
                    colleague: frozen.report.colleague,
                    account: frozen.report.account,
                })),
            },
        },
    }
}

pub(crate) async fn keep(node: &Arc<Node>, goal: &Goal) -> Result<(), Failure> {
    let (node, goal, runtime) = (node.clone(), goal.clone(), tokio::runtime::Handle::current());
    fastctx_platform::task::spawn_blocking(move || keep_now(&node, &goal, runtime)).await.map_err(failure)?
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
