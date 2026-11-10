//! Withdrawal applies to every local update path and survives an offline start.
use crate::Node;
use fastctx_types::update::Problem;

pub(super) fn check(node: &Node, version: &str) -> Result<(), Problem> {
    for link in node.links() {
        if let Some(network) = link.update_network() {
            network.policy.check_release(version)?;
        }
    }
    Ok(())
}
