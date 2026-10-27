//! Call snapshots contain identities and timing, never request contents.

use super::*;
use fastctx_types::stats::CallWork;

pub(crate) fn snapshot() -> Vec<CallWork> {
    let waiting = fastctx_platform::doing::task_waits();
    rows(&state(), &waiting)
}

fn rows(state: &State, waiting: &HashMap<tokio::task::Id, String>) -> Vec<CallWork> {
    let mut rows: Vec<_> = state
        .calls
        .iter()
        .map(|(id, entry)| {
            let seconds = entry.request.received.elapsed().as_secs();
            CallWork {
                id: *id,
                account: entry.request.account.to_string(),
                machine: entry.request.machine.to_string(),
                conversation: entry.request.conversation.clone(),
                kind: entry.request.kind.clone(),
                pool: format!("{:?}", entry.request.pool).to_lowercase(),
                phase: match entry.phase {
                    Phase::Queued => "queued",
                    Phase::Running => "running",
                    Phase::Settling => "settling",
                }
                .into(),
                seconds,
                within: entry.request.within.map(|d| d.as_secs()),
                overdue: entry.request.within.is_some_and(|within| entry.request.received.elapsed() > within),
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
    Some(rows(&state, &waiting))
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

pub(crate) fn stuck() -> Vec<fastctx_platform::doing::Stuck> {
    snapshot()
        .into_iter()
        .filter(|row| row.overdue)
        .map(|row| fastctx_platform::doing::Stuck {
            key: format!("call {}", row.id),
            doing: format!("{} ({})", row.kind, row.phase),
            what: "past its contract".into(),
            since: Duration::from_secs(row.seconds),
        })
        .collect()
}
