//! The one pace of everything that tries again (`world-network.md`,
//! 断开不是结束): a line to the hub or another machine dialed again, a stream
//! an interface opens again, a copy that goes on, words said again under
//! their id. The first try is at once; after that a random wait up to a
//! ceiling that doubles with each failure, to a minute, so what broke
//! together does not all return in the same instant; once it held a while,
//! the next break starts over from at once.
//!
//! A wait ends early when a way to where the work goes opens: the line to
//! the hub came back, the machine it reaches connected anew, a direct link
//! came up (`link::Way`). The waits keep the pace and a way opening only
//! hurries it, so a signal missed costs no more than the wait.

use std::future::Future;
use std::time::Duration;

use tokio::sync::watch;

/// A connection or a stream that stays up this long counts as healthy: the
/// next failure is tried again at once.
pub const STABLE: Duration = Duration::from_secs(60);

/// The longest wait between two tries.
pub const LONGEST: Duration = Duration::from_secs(60);

/// How long to wait before trying again after `failures` failures in a row.
pub fn backoff(failures: u32) -> Duration {
    if failures <= 1 {
        return Duration::ZERO;
    }
    let ceiling_ms = ceiling(failures);
    let random = u64::from_le_bytes(crate::sign::random::<8>().unwrap_or([0; 8]));
    Duration::from_millis(250 + random % ceiling_ms.saturating_sub(250).max(1))
}

/// Whether this retry's upper bound has reached the common maximum.
pub fn at_ceiling(failures: u32) -> bool {
    ceiling(failures) == LONGEST.as_millis() as u64
}

fn ceiling(failures: u32) -> u64 {
    if failures <= 1 {
        return 0;
    }
    1000_u64.saturating_mul(1 << (failures - 2).min(6)).min(LONGEST.as_millis() as u64)
}

/// Resolves once a way opens that was not there when this was asked:
/// `ways` says the ways there are now, and `changes` moves whenever they may
/// have changed. Ways that closed, or that were there all along, do not end
/// it; with no one left to move `changes`, it never resolves.
pub fn reopened<W: PartialEq + Send + 'static>(
    mut changes: watch::Receiver<u64>,
    ways: impl Fn() -> Vec<W> + Send + 'static,
) -> impl Future<Output = ()> + Send + 'static {
    // Marked before the ways are read: a change between the two is seen
    // again, and a way it opened is in `before` already.
    changes.borrow_and_update();
    let before = ways();
    async move {
        loop {
            if changes.changed().await.is_err() {
                return std::future::pending().await;
            }
            if ways().iter().any(|way| !before.contains(way)) {
                return;
            }
        }
    }
}
