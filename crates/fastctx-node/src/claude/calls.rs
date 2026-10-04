//! Proofs of Claude calls whose session is over: a call cancelled or denied
//! before it reached the World leaves its proof behind.

use std::sync::Arc;

use fastctx_hosts::claude::calls;

use crate::Node;

/// Sweeps the proofs of sessions that are no longer present: at start, for
/// sessions that ended while the node was down, and when a session ends.
pub(super) fn sweep(node: &Arc<Node>) {
    match calls::sweep(node.world.path(), |registration| Ok(!super::pairing::present(node, registration)?)) {
        Ok(problems) => {
            for problem in problems {
                node.log.line(format_args!("a Claude call proof stays: {problem}"));
            }
        }
        Err(error) => node.log.line(format_args!("sweeping Claude call proofs: {error}")),
    }
}
