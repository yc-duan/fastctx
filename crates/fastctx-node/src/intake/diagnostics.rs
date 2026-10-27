//! Call snapshots contain identities and timing, never request contents.

use super::*;
use fastctx_types::stats::CallWork;

pub(crate) fn snapshot() -> Vec<CallWork> {
    let waiting = fastctx_platform::doing::task_waits();
    let state = state();
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

pub(crate) fn dump(log: &crate::log::Log) {
    let rows = snapshot();
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
            doing: format!("{} for {} from {} ({})", row.kind, row.account, row.machine, row.phase),
            what: row.waiting,
            since: Duration::from_secs(row.seconds),
        })
        .collect()
}
