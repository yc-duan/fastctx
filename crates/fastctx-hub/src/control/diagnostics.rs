//! Forwarded-call ages are not evidence that the hub itself is stuck.

use fastctx_types::call_diagnostics::{Class, Summary};
use std::{sync::Arc, time::Duration};

pub(super) struct Kind {
    pub name: String,
    pub class: Class,
    pub within: Option<Duration>,
}

impl Kind {
    pub(super) fn of(call: &fastctx_types::remote::RemoteCall) -> Self {
        Self { name: call.kind(), class: call.diagnostic_class(), within: call.contract().within }
    }
}

pub(super) struct Call {
    pub id: u64,
    pub caller: fastctx_types::MachineName,
    pub target: fastctx_types::MachineName,
    pub account: fastctx_types::AccountName,
    pub kind: Option<Kind>,
}

pub(crate) fn snapshot(hub: &crate::Hub) -> serde_json::Value {
    let calls: Vec<_> = hub
        .calls
        .lock()
        .expect("never poisoned")
        .values()
        .map(|pending| (pending.at, Arc::clone(&pending.diagnostic)))
        .collect();
    summarize(calls)
}

fn summarize(calls: Vec<(tokio::time::Instant, Arc<Call>)>) -> serde_json::Value {
    summarize_at(calls, tokio::time::Instant::now())
}

fn summarize_at(calls: Vec<(tokio::time::Instant, Arc<Call>)>, now: tokio::time::Instant) -> serde_json::Value {
    let mut summary = Summary::default();
    let oldest = calls.iter().map(|(at, _)| now.saturating_duration_since(*at).as_secs()).max();
    let mut overdue = Vec::new();
    for (at, call) in &calls {
        let age = now.saturating_duration_since(*at);
        let class = call.kind.as_ref().map_or(Class::Unknown, |kind| kind.class);
        let within = call.kind.as_ref().and_then(|kind| kind.within);
        let late = within.is_some_and(|within| age > within);
        summary.record(class, age.as_secs(), late);
        if late {
            overdue.push((age.as_secs(), call, within));
        }
    }
    overdue.sort_by_key(|(age, _, _)| std::cmp::Reverse(*age));
    let overdue: Vec<_> = overdue
        .into_iter()
        .take(5)
        .map(|(age, call, within)| {
            serde_json::json!({
                "id":call.id, "kind":call.kind.as_ref().map(|kind| &kind.name),
                "from":call.caller, "to":call.target, "account":call.account,
                "phase":"awaiting_target_answer", "seconds":age, "within":within.map(|within| within.as_secs()),
            })
        })
        .collect();
    fastctx_platform::call_readings::set(summary);
    serde_json::json!({
        // Retained for readers of the old health file: these still count all calls.
        "pending":calls.len(), "oldest_seconds":oldest,
        "classified":summary, "oldest_overdue":overdue,
    })
}
