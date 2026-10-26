//! Project membership belongs to the accounts a caller serves, independently
//! of where its host or the project's work happens.

use std::sync::Arc;

use fastctx_types::TownName;
use fastctx_types::access::Acting;
use fastctx_types::ipc::Failure;

use crate::Node;

/// Whether a currently known account of `acting` may use `town`.
/// This reads authorization only; it never reads project content, and
/// what asked is not done when it cannot tell.
pub(crate) fn permits(node: &Node, acting: &Acting, town: &TownName) -> Result<bool, Failure> {
    let place = crate::ledger::names::place(&node.world, town).map_err(|error| Failure::not_run(error.to_string()))?;
    if let Some(place) = place {
        if let Some(link) = node.link_of(&place.hub) {
            let view = link.view();
            return Ok(node.accounts_on(&link, acting).iter().any(|account| view.in_project(account, &place.town)));
        }
        return Ok(place.hub
            == crate::ledger::alone(&node.world).map_err(|error| Failure::not_run(error.to_string()))?
            && matches!(acting, Acting::Here));
    }
    Ok(node.links().iter().any(|link| {
        let view = link.view();
        node.accounts_on(link, acting).iter().any(|account| view.project_named(account, town).is_some())
    }))
}

/// Refuses without naming any other account's machines or project contents.
pub(crate) fn check(node: &Node, acting: &Acting, town: &TownName) -> Result<(), Failure> {
    if permits(node, acting, town)? {
        Ok(())
    } else {
        Err(Failure::refused(format!("the caller's accounts may not use the project {town}")))
    }
}

/// Checks the current project without treating an omitted name as outside.
pub(crate) fn current(node: &Node, resident: &fastctx_types::ResidentId) -> Result<(), Failure> {
    let (about, town) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::refused("the conversation's project identity is unknown"))?;
    if let Some(town) = town {
        check(node, &about.acting, &town)?;
    }
    Ok(())
}

/// Checks access before fetching content, and again after the fetch.
pub(crate) async fn ready(node: &Arc<Node>, acting: &Acting, town: &TownName) -> Result<(), Failure> {
    let place = crate::ledger::names::place(&node.world, town).map_err(|error| Failure::new(error.to_string()))?;
    let links = node.links();
    let known =
        place.is_some() || links.iter().any(|link| link.view().projects.iter().any(|project| project.name == *town));
    for link in links {
        let relevant = match &place {
            Some(place) => place.hub == link.membership().id,
            None => !known || link.view().projects.iter().any(|project| project.name == *town),
        };
        if relevant && !node.accounts_on(&link, acting).is_empty() {
            link.refresh().await.map_err(Failure::new)?;
        }
    }
    check(node, acting, town)?;
    super::map::query(node, town).await?;
    check(node, acting, town)
}
