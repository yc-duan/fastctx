//! What this machine keeps free in memory for its agents (2026-11-16).
//!
//! Claude Code's daemon retires idle sessions in no order once less than a
//! gigabyte is available (its `tengu_bg_low_mem_mb`, measured as
//! `MemAvailable` on Linux): the ones a person used a minute ago among them,
//! while ones done hours ago stay. So the node keeps above that line itself:
//! below `LOW` it lets go of idle sessions, the one whose model acted
//! longest ago first, until `HIGH` is available again; and before a host is
//! woken or started, it frees up to `HIGH` too (`make_room`), so the daemon
//! finds enough as it starts one. A session whose model works is never let
//! go. Nothing waits for memory, and the kernel's own swap is left to it.
//! On Linux the node also ranks its agents' processes for the kernel's OOM
//! killer (`keeper`).

mod keeper;

use std::sync::Arc;
use std::time::Duration;

use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;

use crate::Node;

/// Below this much available, idle sessions are let go.
pub(crate) const LOW: u64 = 1229 << 20;
/// Up to this much: where relief stops, and what a wake frees first.
pub(crate) const HIGH: u64 = 1382 << 20;
/// How often memory is looked at: well within the daemon's minute.
const EVERY: Duration = Duration::from_secs(10);

/// The memory available here now, as the system counts it; none where it
/// cannot be read.
pub(crate) fn available() -> Option<u64> {
    fastctx_platform::system::memory().ok().map(|memory| memory.total.saturating_sub(memory.used))
}

/// Looks at memory every few seconds, letting go of idle sessions while it
/// runs short; and ranks the agents' processes where the system has a way.
pub(crate) fn start(node: &Arc<Node>) {
    keeper::start(node);
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        node.ready().await;
        let mut timer = tokio::time::interval(EVERY);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            if available().is_some_and(|available| available < LOW) {
                crate::lifecycle::relieve(&node, HIGH, None, crate::lifecycle::Asking::Pressed).await;
            }
        }
    });
}

/// Frees memory before `id`'s host is woken or started: up to `HIGH`, by
/// letting go of others that are idle, for a few seconds at most. Several
/// woken at once may each find enough and start together: nothing is kept
/// for one about to start, and the kernel's swap takes what is short.
pub(crate) async fn make_room(node: &Arc<Node>, id: &ResidentId) {
    if available().is_some_and(|available| available < HIGH) {
        crate::lifecycle::relieve(node, HIGH, Some(id), crate::lifecycle::Asking::Waking).await;
    }
}

/// What `machine`, or this computer, keeps free: no more a setting, so an
/// earlier interface asking is told none is set, and setting one is refused.
pub(crate) async fn query(
    node: &Arc<Node>,
    _acting: &fastctx_types::access::Acting,
    _machine: Option<fastctx_types::access::Written>,
    bytes: Option<u64>,
) -> Result<u64, Failure> {
    answer(node, bytes)
}

/// As `query`, for this machine.
pub(crate) fn answer(_node: &Arc<Node>, bytes: Option<u64>) -> Result<u64, Failure> {
    match bytes {
        None => Ok(0),
        Some(_) => {
            Err(Failure::refused("this machine frees memory for its agents itself now; there is nothing to set"))
        }
    }
}

/// Bytes as a person reads them, for the log.
pub(crate) fn size(bytes: u64) -> String {
    const GIB: f64 = (1u64 << 30) as f64;
    const MIB: f64 = (1u64 << 20) as f64;
    match bytes {
        bytes if bytes >= 1 << 30 => format!("{:.2} GiB", bytes as f64 / GIB),
        bytes => format!("{:.0} MiB", bytes as f64 / MIB),
    }
}
