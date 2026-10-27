//! The node watching itself (`specs/world-concurrency.md`, 节点看着自己).
//!
//! Every few seconds it looks at what its tasks wait for and hold
//! (`fastctx_platform::doing`): what has gone on too long goes in its log,
//! once and then again every ten minutes while it lasts, and is kept for its
//! hubs and interfaces to show. Tasks that wait on each other in a circle
//! cannot go on by themselves; one that is still there a minute later ends
//! the node, for its service to start it again. It looks from a thread of
//! its own: a task would wait for the very worker threads whose blocking it
//! is there to see.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_platform::doing;

use crate::Node;

/// How often the node looks at itself.
const LOOK: Duration = Duration::from_secs(10);
/// A wait for a lock longer than this is told.
const WAITS: Duration = Duration::from_secs(30);
/// A lock held longer than this is told.
const HOLDS: Duration = Duration::from_secs(600);
/// What is still stuck is told again after this long.
const AGAIN: Duration = Duration::from_secs(600);
/// A circle still there this long after it was first seen ends the node.
const CIRCLE: Duration = Duration::from_secs(60);

/// What of the node's work is stuck now, as last looked at.
#[derive(Default)]
pub(crate) struct Stuck {
    now: Mutex<Vec<doing::Stuck>>,
}

impl Stuck {
    /// What is stuck now: what its task does and waits for, and how long.
    pub(crate) fn now(&self) -> Vec<doing::Stuck> {
        self.now.lock().expect("never poisoned").clone()
    }
}

/// Tells the order of locks' slips to the node's log, and starts looking.
pub(crate) fn start(node: &Arc<Node>) {
    let log = node.log.clone();
    doing::set_reporter(move |line| log.line(line));
    let watched = Arc::downgrade(node);
    let spawned = std::thread::Builder::new().name("node-stuck".to_string()).spawn(move || {
        let mut told: HashMap<String, Instant> = HashMap::new();
        let mut circle: Option<(String, Instant)> = None;
        let mut handoff_reported: Option<Instant> = None;
        loop {
            std::thread::sleep(LOOK);
            // The node is gone when its last holder let go: nothing to watch.
            let Some(node) = watched.upgrade() else { return };
            let mut snapshot = doing::snapshot(WAITS, HOLDS);
            snapshot.stuck.extend(crate::intake::stuck());
            let now = Instant::now();
            if !node.handoff.yielding() {
                handoff_reported = None;
            } else if !snapshot.active.is_empty()
                && handoff_reported.is_none_or(|last| now.duration_since(last) >= AGAIN)
            {
                let mut active: Vec<_> = snapshot
                    .active
                    .iter()
                    .map(|work| {
                        format!("{}: {} ({} s)", work.what, work.doing, now.duration_since(work.since).as_secs())
                    })
                    .collect();
                active.sort();
                node.log.line(format_args!("handoff waits for {} active tasks: {}", active.len(), active.join("; ")));
                handoff_reported = Some(now);
            }
            told.retain(|key, _| snapshot.stuck.iter().any(|stuck| &stuck.key == key));
            for stuck in &snapshot.stuck {
                let due = told.get(&stuck.key).is_none_or(|at| now.duration_since(*at) >= AGAIN);
                if due {
                    told.insert(stuck.key.clone(), now);
                    node.log.line(format_args!(
                        "stuck for {} s: {} {}",
                        stuck.since.as_secs(),
                        stuck.doing,
                        stuck.what
                    ));
                }
            }
            *node.stuck.now.lock().expect("never poisoned") = snapshot.stuck;
            let Some(first) = snapshot.cycles.first() else {
                circle = None;
                continue;
            };
            let mut links = first.links.clone();
            links.sort();
            let key = links.join("; ");
            match &circle {
                Some((seen, since)) if *seen == key && now.duration_since(*since) >= CIRCLE => {
                    crate::ending::with_reason(
                        1,
                        format!(
                            "tasks have waited on each other in a circle for {} s, so none of them can go on: {key}; \
                         ending this node for its service to start it again",
                            now.duration_since(*since).as_secs()
                        ),
                    );
                }
                Some((seen, _)) if *seen == key => {}
                _ => {
                    node.log.line(format_args!("tasks wait on each other in a circle: {key}"));
                    circle = Some((key, now));
                }
            }
        }
    });
    if let Err(error) = spawned {
        node.log.line(format_args!("starting to watch this node: {error}"));
    }
}
