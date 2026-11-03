//! Call snapshots contain identities and timing, never request contents.

use super::*;
use fastctx_types::stats::CallWork;

pub(crate) fn snapshot() -> Vec<CallWork> {
    let waiting = fastctx_platform::doing::task_waits();
    let mut calls = rows(&state(), &waiting);
    calls.extend(crate::local_subscriptions::snapshot());
    calls.sort_by_key(|call| call.id);
    calls
}

fn rows(state: &State, waiting: &HashMap<tokio::task::Id, String>) -> Vec<CallWork> {
    rows_at(state, waiting, tokio::time::Instant::now())
}

fn rows_at(state: &State, waiting: &HashMap<tokio::task::Id, String>, now: tokio::time::Instant) -> Vec<CallWork> {
    let mut rows: Vec<_> = state
        .calls
        .iter()
        .map(|(id, entry)| {
            let seconds = now.saturating_duration_since(entry.request.received).as_secs();
            CallWork {
                id: *id,
                account: entry.request.account.to_string(),
                machine: entry.request.machine.to_string(),
                conversation: entry.request.conversation.clone(),
                kind: entry.request.kind.clone(),
                class: Some(entry.request.class),
                pool: format!("{:?}", entry.request.pool).to_lowercase(),
                phase: match entry.phase {
                    Phase::Queued => "queued",
                    Phase::Running => "running",
                    Phase::Settling => "settling",
                }
                .into(),
                seconds,
                within: entry.request.within.map(|d| d.as_secs()),
                overdue: entry
                    .request
                    .within
                    .is_some_and(|within| now.saturating_duration_since(entry.request.received) > within),
                waiting: entry.task.and_then(|task| waiting.get(&task).cloned()).unwrap_or_else(|| {
                    match entry.phase {
                        Phase::Queued => "waiting for admission",
                        Phase::Running => "answering",
                        Phase::Settling => "finishing after the caller left",
                    }
                    .into()
                }),
            }
        })
        .collect();
    rows.sort_by_key(|row| row.id);
    rows
}

fn try_snapshot() -> Option<Vec<CallWork>> {
    let waiting = fastctx_platform::doing::try_task_waits()?;
    let state = STATE.try_lock().ok()?;
    let mut calls = rows(&state, &waiting);
    drop(state);
    calls.extend(crate::local_subscriptions::try_snapshot()?);
    calls.sort_by_key(|call| call.id);
    Some(calls)
}

pub(crate) fn dump(log: &crate::log::Log) {
    let Some(rows) = try_snapshot() else {
        log.line("the in-progress call registry is busy; exiting without a snapshot");
        return;
    };
    log.line(format_args!("node ending with {} calls in progress", rows.len()));
    for row in rows {
        log.line(format_args!("call in progress: {}", serde_json::to_string(&row).expect("call diagnostic")));
    }
}

/// One census supplies both public redacted descriptions and local evidence.
pub(crate) fn stuck() -> (Vec<fastctx_platform::doing::Stuck>, std::collections::HashMap<String, String>) {
    let calls = snapshot();
    let mut summary = fastctx_types::call_diagnostics::Summary::default();
    let mut stuck = Vec::new();
    let mut details = std::collections::HashMap::new();
    for call in calls {
        summary.record(call.class.unwrap_or_default(), call.seconds, call.overdue);
        if !call.overdue {
            continue;
        }
        let key = format!("call {}", call.id);
        details.insert(key.clone(), serde_json::to_string(&call).expect("call diagnostic"));
        stuck.push(fastctx_platform::doing::Stuck {
            key,
            doing: format!("{} ({})", call.kind, call.phase),
            what: "past its contract".into(),
            since: Duration::from_secs(call.seconds),
        });
    }
    fastctx_platform::call_readings::set(summary);
    (stuck, details)
}
