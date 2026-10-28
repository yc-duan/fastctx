//! Accepted conclusions are immutable facts, queued before an operation releases its proposed state.

use crate::Node;
use fastctx_types::Address;
use fastctx_types::goals::{Goal, Owner, Report, ReportSummary, Status, StopKind};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::ConversationDir;
use fastctx_types::residents::{Delivery, Notice, Origin, Owing};
use serde::{Deserialize, Serialize};
use std::io;
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

#[derive(Serialize, Deserialize)]
struct Frozen {
    to: Address,
    report: Report,
}

fn directory(conversation: &ConversationDir) -> std::path::PathBuf {
    conversation.books().join("goal-reports")
}

/// Called for both accepted intents and authoritative local views. A later edit or resume
/// cannot replace the report owed by an earlier accepted intent.
pub(crate) fn keep(node: &Arc<Node>, goal: &Goal) -> Result<(), Failure> {
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
    let path = directory(&conversation).join(format!("{id}.json"));
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
    fastctx_platform::ensure_private_dir(path.parent().expect("goal report directory")).map_err(failure)?;
    match fastctx_platform::publish_new(
        &path,
        &serde_json::to_vec(&proposed).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    ) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(failure(error)),
        _ => {}
    }
    let frozen: Frozen = serde_json::from_slice(&std::fs::read(&path).map_err(failure)?).map_err(failure)?;
    if frozen.report.id != id || frozen.report.goal != goal.id || frozen.report.account != goal.account {
        return Err(Failure::new(super::words::REPORT_CHANGED));
    }
    crate::inbox::owe(
        &conversation,
        &Owing {
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
        },
    )
    .map_err(failure)?;
    let node = node.clone();
    let resident = goal.resident.resident.clone();
    tokio::spawn(async move { crate::turns::pay(&node, &resident, &conversation).await });
    Ok(())
}

pub(crate) fn recent(conversation: &ConversationDir) -> io::Result<Vec<ReportSummary>> {
    let entries = match std::fs::read_dir(directory(conversation)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut reports = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "json")
            || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let Frozen { report, .. } = serde_json::from_slice(&std::fs::read(&path)?)?;
        reports.push(ReportSummary {
            id: report.id,
            account: report.account,
            status: report.status,
            ended_at: report.ended_at,
            elapsed_seconds: report.ended_at.as_second().saturating_sub(report.started_at.as_second()).max(0) as u64,
            preview: report.text.lines().next().unwrap_or_default().chars().take(240).collect(),
            path: path.to_string_lossy().into_owned(),
        });
    }
    reports.sort_by(|left, right| left.ended_at.cmp(&right.ended_at).then(left.id.cmp(&right.id)));
    reports.drain(..reports.len().saturating_sub(10));
    Ok(reports)
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
