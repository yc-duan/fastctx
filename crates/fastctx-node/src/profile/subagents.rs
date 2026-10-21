//! The Claude subagents of a conversation's account (`world-user-config.md`),
//! as its plugin reads them at each turn's start: the account's own, or this
//! version's defaults where it set none. Never a login's own directory.

use crate::Node;
use fastctx_types::{access::Acting, ipc::Failure, profile::Subagents, residents::About};

/// The account's subagents for a Claude conversation working for one
/// account, as the hub last told this machine; none for any other.
pub(crate) fn configuration(node: &Node, about: &About) -> Result<Option<Subagents>, Failure> {
    if about.host.agent.as_str() != "claude" {
        return Ok(None);
    }
    let Acting::Accounts { accounts } = &about.acting else { return Ok(None) };
    let [account] = accounts.as_slice() else { return Ok(None) };
    let link =
        node.link_of(&account.hub).ok_or_else(|| Failure::new("the conversation's account is no longer connected"))?;
    let profile = link.profile_read(&account.account).map_err(Failure::new)?;
    profile.profile.subagents.unwrap_or_default().for_session().map(Some).map_err(Failure::new)
}

/// What the plugin of the Claude session `session` registers: its account's
/// subagents, with the user's instructions in its directory brought up to
/// date on the way.
pub(crate) async fn for_session(node: &Node, session: &str) -> Result<Option<Subagents>, Failure> {
    let id = node
        .residents
        .resident_of_thread(session)
        .ok_or_else(|| Failure::new("the Claude session has not been paired"))?;
    let (about, _) = node.residents.about(&id).ok_or_else(|| Failure::new("unknown conversation"))?;
    node.residents.active(&id)?;
    if about.host.agent.as_str() != "claude" {
        return Err(Failure::refused("this is not a Claude session"));
    }
    super::session::ready(node, &about.acting).await?;
    let directory = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
    super::session::hand_to_session(node, &id, &directory).map_err(|e| Failure::new(e.to_string()))?;
    configuration(node, &about)
}
