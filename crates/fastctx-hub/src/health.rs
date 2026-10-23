//! How the hub is doing, for whoever looks after it (`world-concurrency.md`,
//! 中枢看着自己): every half minute it writes `health.json` beside its
//! records, and every five minutes a line of it in its log. What it counts
//! is taken under each lock in a short look and summed off it; what it says
//! of members is of the few with the most waiting, so it stays small however
//! large the network.

use std::collections::VecDeque;
use std::sync::{Mutex, Weak};
use std::time::Duration;

use fastctx_platform::{Permissions, publish};
use serde_json::json;
use tokio::time::Instant;

use crate::Hub;

/// Where it is written, in the hub's directory.
pub(crate) const FILE: &str = "health.json";
/// How often it is written, unless `EVERY_VARIABLE` says otherwise.
const EVERY: Duration = Duration::from_secs(30);
/// Names a whole number of seconds to write it every instead, for whoever
/// watches the hub closely: the network drill reads it after each scenario.
const EVERY_VARIABLE: &str = "FASTCTX_HUB_HEALTH_SECONDS";
/// How often a line of it goes in the log.
const LOG_EVERY: Duration = Duration::from_secs(300);
/// How long the comings of sessions are counted over.
const RECENT: Duration = Duration::from_secs(300);
/// How many of the latest letters kept are timed.
const TIMED: usize = 4096;
/// How many members it names, of those with the most waiting.
const NAMED: usize = 5;

/// What the hub counts as it goes, for its health.
#[derive(Default)]
pub(crate) struct Health {
    /// How long each of the latest letters took to keep, in milliseconds.
    kept: Mutex<VecDeque<u32>>,
    /// When sessions opened lately.
    opened: Mutex<VecDeque<Instant>>,
}

impl Health {
    /// A letter was kept, taking `took`.
    pub(crate) fn kept(&self, took: Duration) {
        let mut kept = self.kept.lock().expect("never poisoned");
        if kept.len() == TIMED {
            kept.pop_front();
        }
        kept.push_back(u32::try_from(took.as_millis()).unwrap_or(u32::MAX));
    }

    /// A member's session opened.
    pub(crate) fn opened(&self) {
        let now = Instant::now();
        let mut opened = self.opened.lock().expect("never poisoned");
        while opened.front().is_some_and(|at| now.duration_since(*at) > RECENT) {
            opened.pop_front();
        }
        opened.push_back(now);
    }
}

/// Writes the hub's health while the hub runs.
pub(crate) async fn keep(hub: Weak<Hub>) {
    let every = std::env::var(EVERY_VARIABLE)
        .ok()
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map_or(EVERY, Duration::from_secs);
    let mut logged: Option<Instant> = None;
    loop {
        tokio::time::sleep(every).await;
        let Some(hub) = hub.upgrade() else { return };
        let now = now(&hub);
        let path = hub.store().dir().join(FILE);
        let mut bytes = serde_json::to_vec_pretty(&now).expect("health serializes");
        bytes.push(b'\n');
        if let Err(error) = publish(&path, &bytes, Permissions::Private) {
            hub.log.line(format_args!("writing how the hub is doing: {error}"));
        }
        if logged.is_none_or(|at| at.elapsed() >= LOG_EVERY) {
            logged = Some(Instant::now());
            hub.log.line(format_args!("health: {}", serde_json::to_string(&now).expect("health serializes")));
        }
    }
}

/// How the hub is doing now.
fn now(hub: &Hub) -> serde_json::Value {
    let queues: Vec<(String, crate::outq::Sizes)> =
        hub.sessions().iter().map(|(name, session)| (name.to_string(), session.out.sizes())).collect();
    let oldest = hub.calls.lock().expect("never poisoned").values().map(|pending| pending.at).min();
    let (waiting, live, registered) = crate::data::counts(hub);
    let (letters, behind) = {
        let store = hub.store();
        (crate::mailbox::backlog(&store), store.revision().saturating_sub(store.records.written_revision()))
    };
    let kept: Vec<u32> = hub.health.kept.lock().expect("never poisoned").iter().copied().collect();
    let opened = {
        let now = Instant::now();
        let opened = hub.health.opened.lock().expect("never poisoned");
        opened.iter().filter(|at| now.duration_since(**at) <= RECENT).count()
    };

    let mut fullest = queues.clone();
    fullest.sort_by_key(|(_, sizes)| std::cmp::Reverse(sizes.urgent_bytes + sizes.letters + sizes.notices));
    let mut backlog = letters.clone();
    backlog.sort_by_key(|waiting| std::cmp::Reverse(waiting.held));
    json!({
        "at": jiff::Timestamp::now(),
        "sessions": queues.len(),
        "opened_in_5_minutes": opened,
        "queues": {
            "urgent_frames": queues.iter().map(|(_, sizes)| sizes.urgent).sum::<usize>(),
            "urgent_bytes": queues.iter().map(|(_, sizes)| sizes.urgent_bytes).sum::<usize>(),
            "letters": queues.iter().map(|(_, sizes)| sizes.letters).sum::<usize>(),
            "notices": queues.iter().map(|(_, sizes)| sizes.notices).sum::<usize>(),
            "fullest": fullest.iter().filter(|(_, sizes)| sizes.urgent + sizes.letters + sizes.notices > 0).take(NAMED).map(|(name, sizes)| json!({
                "member": name,
                "urgent_frames": sizes.urgent,
                "urgent_bytes": sizes.urgent_bytes,
                "letters": sizes.letters,
                "notices": sizes.notices,
            })).collect::<Vec<_>>(),
        },
        "calls": {
            "pending": hub.calls.lock().expect("never poisoned").len(),
            "oldest_seconds": oldest.map(|at| at.elapsed().as_secs()),
        },
        "transfers": { "waiting": waiting, "live": live, "registered": registered },
        "letters": {
            "held": letters.iter().map(|waiting| waiting.held).sum::<usize>(),
            "being_written": letters.iter().map(|waiting| waiting.writing).sum::<usize>(),
            "on_their_way": letters.iter().map(|waiting| waiting.on_way).sum::<usize>(),
            "records_behind": behind,
            "most_waiting": backlog.iter().take(NAMED).filter(|waiting| waiting.held > 0).map(|waiting| json!({
                "member": waiting.member.to_string(),
                "held": waiting.held,
                "on_their_way": waiting.on_way,
            })).collect::<Vec<_>>(),
        },
        "keeping_a_letter_ms": percentiles(kept),
        "packages": crate::updates::health(hub),
        "account_records": hub.account_records.health(),
    })
}

/// The middle, the 95th and the 99th hundredth of `times`, and how many.
fn percentiles(mut times: Vec<u32>) -> serde_json::Value {
    if times.is_empty() {
        return json!({ "count": 0 });
    }
    times.sort_unstable();
    let at = |share: usize| times[(times.len() - 1) * share / 100];
    json!({ "count": times.len(), "p50": at(50), "p95": at(95), "p99": at(99) })
}
