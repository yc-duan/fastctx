//! A managed machine (`world-network.md`, 受管机器): this computer, registered
//! by a code under an account, logged in to none. Others' agents run
//! commands here; nothing of the World serves anyone sitting at it. It
//! scans no agent and puts the World's tools in none, takes no session of
//! its own agents for a conversation, opens none, and keeps no project's
//! records. Only a World never used becomes one: no network, no agent with
//! the World's tools, no conversation.

use fastctx_types::access::HubId;
use fastctx_types::accounts::Unfit;
use fastctx_types::ipc::Failure;

use crate::Node;

/// Whether this computer is a managed machine on any of its hubs.
pub(crate) fn is_managed(node: &Node) -> bool {
    node.links().iter().any(|link| link.view().me().is_some_and(|me| me.managed))
}

/// Refuses what a managed machine does not do for whoever sits at it.
pub(crate) fn refuse(node: &Node) -> Result<(), Failure> {
    if is_managed(node) {
        return Err(Failure::not_run(format!(
            "{} is a managed machine: agents elsewhere work on it, and nothing of the World serves this computer itself",
            node.machine()
        )));
    }
    Ok(())
}

/// Why this World may not become a managed machine on `hub`, if it may not:
/// it is on another network, has conversations, or the World's tools are in
/// an agent here. On `hub` itself it may be one already, as when a first
/// try was cut off.
pub(crate) fn unfit(node: &Node, hub: &HubId) -> Option<Unfit> {
    if node.links().iter().any(|link| link.hub() != hub) {
        return Some(Unfit::Network);
    }
    if node.residents.everyone().next().is_some() {
        return Some(Unfit::Conversations);
    }
    let applied = crate::logins::Context::of(node)
        .map_err(|failure| failure.message)
        .and_then(|context| fastctx_install::apply::integrations(&context.places).map_err(|error| error.to_string()));
    match applied {
        Ok(integrations) if integrations.is_empty() => None,
        Ok(_) => Some(Unfit::Integrated),
        Err(detail) => Some(Unfit::Unknown { detail }),
    }
}
