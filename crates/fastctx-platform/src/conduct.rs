//! What this process did against its own rules (`specs/world-foundation.md`,
//! 规矩由机器守): work that blocked a thread that must not block, a task that
//! kept its thread too long at once, a runtime whose tasks waited too long
//! for a thread, a lock over many units held too long.
//!
//! Each kind at each place is counted, with its latest occurrence. A
//! released build only counts: on someone's machine, stopping is worse than
//! the violation. A development build stops at the first violation when
//! `FASTCTX_STRICT_ROLES=1`, as the drills run it.

use std::collections::BTreeMap;
use std::panic::Location;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use fastctx_types::readings::Violation;

use crate::roles::Role;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Blocking,
    SlowPoll,
    HeldUp,
    HeldLong,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Blocking => "blocking",
            Kind::SlowPoll => "slow-poll",
            Kind::HeldUp => "held-up",
            Kind::HeldLong => "held-long",
        }
    }
}

struct Seen {
    count: u64,
    doing: String,
    worst: Duration,
    last: jiff::Timestamp,
}

/// Places kept; past it, a new place is only counted in the total.
const PLACES: usize = 512;

static TOTAL: AtomicU64 = AtomicU64::new(0);
static SEEN: LazyLock<Mutex<BTreeMap<(Kind, String), Seen>>> = LazyLock::new(Default::default);

fn strict() -> bool {
    static STRICT: OnceLock<bool> = OnceLock::new();
    *STRICT.get_or_init(|| cfg!(debug_assertions) && std::env::var_os("FASTCTX_STRICT_ROLES").is_some_and(|v| v == "1"))
}

fn note(kind: Kind, place: String, doing: String, took: Duration) {
    TOTAL.fetch_add(1, Ordering::Relaxed);
    let mut seen = SEEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let full = seen.len() >= PLACES;
    let now = jiff::Timestamp::now();
    match seen.get_mut(&(kind, place.clone())) {
        Some(entry) => {
            entry.count += 1;
            entry.doing = doing;
            entry.worst = entry.worst.max(took);
            entry.last = now;
        }
        None if !full => {
            seen.insert((kind, place), Seen { count: 1, doing, worst: took, last: now });
        }
        None => {}
    }
}

/// A thread of `role`, which must not block, was about to block on `what`.
pub(crate) fn blocked(role: Role, what: &'static str, at: &Location<'_>) {
    let place = format!("{what} at {}:{} on a {} thread", at.file(), at.line(), role.word());
    let doing = match crate::doing::said_label() {
        Some(label) => label.to_string(),
        None => format!("unlabeled work on a {} thread", role.word()),
    };
    if strict() {
        panic!("{place}, while {doing}: this thread must not block (specs/world-engine.md)");
    }
    note(Kind::Blocking, place, doing, Duration::ZERO);
}

/// A task kept its thread for `took` at once.
pub fn slow_poll(doing: &str, took: Duration) {
    note(Kind::SlowPoll, doing.to_string(), doing.to_string(), took);
}

/// The tasks of `runtime` waited `by` for a thread.
pub fn held_up(runtime: &str, by: Duration) {
    note(Kind::HeldUp, runtime.to_string(), String::new(), by);
}

/// The lock `lock`, which is over many units, was held for `took` at once
/// by a task or thread doing `doing`.
pub(crate) fn held_long(lock: &str, doing: &str, took: Duration) {
    note(Kind::HeldLong, lock.to_string(), doing.to_string(), took);
}

/// Every violation since the process started.
pub fn total() -> u64 {
    TOTAL.load(Ordering::Relaxed)
}

/// Each kind at each place, the most frequent first.
pub fn snapshot() -> Vec<Violation> {
    let seen = SEEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut violations: Vec<Violation> = seen
        .iter()
        .map(|((kind, place), seen)| Violation {
            kind: kind.word().to_string(),
            place: place.clone(),
            count: seen.count,
            doing: seen.doing.clone(),
            worst_ms: u64::try_from(seen.worst.as_millis()).unwrap_or(u64::MAX),
            last: seen.last,
        })
        .collect();
    violations.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.place.cmp(&b.place)));
    violations
}
