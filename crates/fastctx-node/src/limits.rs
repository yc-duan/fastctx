//! How far this machine is toward its system's limits over the last day
//! (`world-gui.md`, 机器页): read once a minute, kept in memory and every ten
//! minutes in `limits.json`, so that a restart keeps the day; told with how
//! the machine is doing. A production node once ran out of inotify instances
//! and nothing showed it coming.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_platform::limits::Kind;
use fastctx_types::stats::{LimitKind, LimitReading, LimitUse};
use jiff::{SignedDuration, Timestamp};

use crate::Node;

const EVERY: Duration = Duration::from_secs(60);
/// How long a reading is kept.
const KEEP: SignedDuration = SignedDuration::from_hours(24);
/// How many readings go between two keepings on disk.
const KEPT_EVERY: u32 = 10;

#[derive(Default)]
pub(crate) struct Readings(Mutex<VecDeque<LimitReading>>);

fn file(node: &Node) -> std::path::PathBuf {
    node.world.path().join("limits.json")
}

pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        if let Ok(bytes) = std::fs::read(file(&node))
            && let Ok(kept) = serde_json::from_slice::<Vec<LimitReading>>(&bytes)
        {
            let since = Timestamp::now() - KEEP;
            node.limits.0.lock().expect("never poisoned").extend(kept.into_iter().filter(|reading| reading.at > since));
        }
        let mut interval = tokio::time::interval(EVERY);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut unkept = 0;
        loop {
            interval.tick().await;
            let Ok(uses) = tokio::task::spawn_blocking(fastctx_platform::limits::read).await else { continue };
            // A system that bounds none of these has nothing to show.
            if uses.is_empty() {
                continue;
            }
            let at = Timestamp::now();
            let reading = LimitReading { at, uses: uses.into_iter().map(told).collect() };
            {
                let mut readings = node.limits.0.lock().expect("never poisoned");
                readings.push_back(reading);
                while readings.front().is_some_and(|oldest| oldest.at <= at - KEEP) {
                    readings.pop_front();
                }
            }
            unkept += 1;
            if unkept >= KEPT_EVERY {
                unkept = 0;
                let readings: Vec<LimitReading> =
                    node.limits.0.lock().expect("never poisoned").iter().cloned().collect();
                let kept = serde_json::to_vec(&readings).map_err(std::io::Error::other).and_then(|bytes| {
                    fastctx_platform::publish(&file(&node), &bytes, fastctx_platform::Permissions::Private)
                });
                if let Err(error) = kept {
                    node.log.line(format_args!("keeping the readings of this machine's limits: {error}"));
                }
            }
        }
    });
}

/// The readings after `after`, or the whole day's.
pub(crate) fn since(node: &Node, after: Option<Timestamp>) -> Vec<LimitReading> {
    let readings = node.limits.0.lock().expect("never poisoned");
    readings.iter().filter(|reading| after.is_none_or(|after| reading.at > after)).cloned().collect()
}

fn told(use_: fastctx_platform::limits::Use) -> LimitUse {
    let kind = match use_.kind {
        Kind::InotifyInstances => LimitKind::InotifyInstances,
        Kind::InotifyWatches => LimitKind::InotifyWatches,
        Kind::OpenFiles => LimitKind::OpenFiles,
        Kind::Tasks => LimitKind::Tasks,
    };
    LimitUse { kind, used: use_.used, limit: use_.limit }
}
