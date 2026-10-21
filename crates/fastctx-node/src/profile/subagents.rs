//! Account-scoped Claude definitions read afresh at each turn boundary.

use crate::Node;
use fastctx_types::{access::Acting, ipc::Failure, profile::Subagents, residents::About};

pub(crate) const CHANGED: &str = "=== the account's Claude subagent instructions changed; they now read as follows, replacing the previous instructions ===";

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
    // A message can overtake the hub's profile notification on another link.
    // Read its current view before this turn registers the account's agents.
    if let Acting::Accounts { accounts } = &about.acting
        && let [account] = accounts.as_slice()
    {
        let link = node
            .link_of(&account.hub)
            .ok_or_else(|| Failure::new("the conversation's account is no longer connected"))?;
        link.refresh().await.map_err(Failure::new)?;
    }
    super::session::ready(node, &about.acting).await?;
    let directory = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
    super::session::hand_to_session(node, &id, &directory).map_err(|e| Failure::new(e.to_string()))?;
    configuration(node, &about)
}
