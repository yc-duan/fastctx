//! Observe local IPC subscription lifetimes without taking admission capacity.

use fastctx_types::{Call, MachineName, call_diagnostics::Class, stats::CallWork};
use std::{
    collections::BTreeMap,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);
static OPEN: LazyLock<Mutex<BTreeMap<u64, Entry>>> = LazyLock::new(Default::default);

/// Local observations and admitted remote calls share diagnostic identifiers.
pub(crate) fn next_id() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed) + 1
}

struct Entry {
    machine: MachineName,
    kind: String,
    caller: String,
    at: tokio::time::Instant,
}

pub(crate) struct Subscription(u64);

impl Drop for Subscription {
    fn drop(&mut self) {
        OPEN.lock().expect("never poisoned").remove(&self.0);
    }
}

pub(crate) fn observe(machine: MachineName, caller: &str, call: &Call) -> Option<Subscription> {
    if !matches!(
        call,
        Call::WatchMany
            | Call::Watch { .. }
            | Call::WatchWorld { .. }
            | Call::WatchFeatures
            | Call::WatchTools { .. }
            | Call::WatchAccountRecords { .. }
            | Call::WatchHistory { .. }
            | Call::WatchMemory { .. }
            | Call::Lines {
                request: fastctx_types::lines::Request::Open { .. } | fastctx_types::lines::Request::Changed { .. },
                ..
            }
            | Call::FindFiles { request: fastctx_types::file_search::FileSearchRequest::Open { .. }, .. }
    ) {
        return None;
    }
    let id = next_id();
    let entry = Entry {
        machine,
        kind: crate::labels::kind_of(call),
        caller: caller.to_string(),
        at: tokio::time::Instant::now(),
    };
    OPEN.lock().expect("never poisoned").insert(id, entry);
    Some(Subscription(id))
}

fn rows(open: &BTreeMap<u64, Entry>) -> Vec<CallWork> {
    let now = tokio::time::Instant::now();
    open.iter()
        .map(|(id, entry)| CallWork {
            id: *id,
            account: String::new(),
            machine: entry.machine.to_string(),
            conversation: None,
            kind: entry.kind.clone(),
            pool: "stream".into(),
            class: Some(Class::Subscription),
            phase: "local_subscription".into(),
            seconds: now.saturating_duration_since(entry.at).as_secs(),
            within: None,
            overdue: false,
            waiting: format!("watching for {}", entry.caller),
        })
        .collect()
}

pub(crate) fn snapshot() -> Vec<CallWork> {
    rows(&OPEN.lock().expect("never poisoned"))
}
pub(crate) fn try_snapshot() -> Option<Vec<CallWork>> {
    let open = OPEN.try_lock().ok()?;
    Some(rows(&open))
}
