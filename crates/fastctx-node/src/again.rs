//! How the node paces what it tries again (`world-network.md`, 断开不是结束):
//! as everything that reconnects (`fastctx_net::again`), at once, then
//! after waits that double up to a minute. Work that reaches other machines
//! goes again at once when a way to one of them opens (`Node::reopened`):
//! the waits keep the pace, and a way opening only hurries it.

use std::sync::Arc;
use std::time::Duration;

use crate::Node;
use crate::reach::Written;

/// The tries of one piece of work, failed in a row.
#[derive(Default)]
pub(crate) struct Again {
    failures: u32,
}

impl Again {
    /// The wait before the next try of work on this machine alone.
    pub(crate) fn next(&mut self) -> Duration {
        self.failures = self.failures.saturating_add(1);
        fastctx_net::again::backoff(self.failures)
    }

    /// Waits before the next try of work that reaches `machines`: the wait
    /// `next` gives, or until a way to one of them opens.
    pub(crate) async fn pause(&mut self, node: &Arc<Node>, machines: &[Written]) {
        let opened = node.reopened(machines.to_vec());
        tokio::select! {
            () = tokio::time::sleep(self.next()) => {}
            () = opened => {}
        }
    }

    /// The work went on: the next break is tried again at once.
    pub(crate) fn reset(&mut self) {
        self.failures = 0;
    }
}
