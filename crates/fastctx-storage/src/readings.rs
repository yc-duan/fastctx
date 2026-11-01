//! A database's measures among the process's readings, and its write queue
//! among the bounds its health tells of, under the name of its file:
//! `storage.node.write`, `storage.hub.queued` (`specs/world-engine.md`,
//! 量与告警).

use fastctx_platform::health::Bound;
use fastctx_platform::readings::{Counter, Gauge, Timing};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;

pub(crate) struct Readings {
    /// The name its measures share, `storage.<file>`.
    pub name: &'static str,
    /// One operation's SQL on the writer.
    pub write: &'static Timing,
    /// A batch's commit, its flush to disk included.
    pub commit: &'static Timing,
    pub read: &'static Timing,
    pub operations: &'static Counter,
    pub commits: &'static Counter,
    pub failures: &'static Counter,
    /// Operations over the two milliseconds a write may take.
    pub over_budget: &'static Counter,
    /// Operations over twenty milliseconds.
    pub slow: &'static Counter,
    /// Damage found: by the background check or by SQLite in an operation.
    pub damaged: &'static Counter,
    pub queued: &'static Gauge,
    pub size: &'static Gauge,
    pub log_size: &'static Gauge,
    /// The admission slots of the busiest priority.
    pub writes: &'static Bound,
}

static NAMES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

/// `name`, kept for the life of the process once: a process opens a few
/// databases, each under the name of its file, and opens one again at most
/// a few times.
fn interned(name: String) -> &'static str {
    let mut names = NAMES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(name) = names.get(name.as_str()) {
        return name;
    }
    let name: &'static str = Box::leak(name.into_boxed_str());
    names.insert(name);
    name
}

impl Readings {
    pub fn of(path: &Path) -> Self {
        let file = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("database");
        let name = |measure: &str| interned(format!("storage.{file}.{measure}"));
        use fastctx_platform::readings::{counter, gauge, timing};
        Self {
            name: interned(format!("storage.{file}")),
            write: timing(name("write")),
            commit: timing(name("commit")),
            read: timing(name("read")),
            operations: counter(name("operations")),
            commits: counter(name("commits")),
            failures: counter(name("failures")),
            over_budget: counter(name("over_budget")),
            slow: counter(name("slow")),
            damaged: counter(name("damaged")),
            queued: gauge(name("queued")),
            size: gauge(name("bytes")),
            log_size: gauge(name("log_bytes")),
            writes: fastctx_platform::health::bound(name("writes")),
        }
    }
}
