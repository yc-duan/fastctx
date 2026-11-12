//! Which networks the asset library is on (the user, 2026-11-12: what a
//! network had on a machine goes when the machine leaves it). The library
//! keeps, beside its scopes, whether the machine is on each network and the
//! term it is on it under (`fastctx_content::v2::Network`); this keeps that
//! in step with the memberships on the disk. A membership begun or ended
//! changes it in the same step, one membership change at a time
//! (`world::memberships`); at start it follows the memberships the computer
//! kept. Each network's term is also kept here in memory for the producers
//! that ask where to write; the library checks it again as it writes.

use super::session::blocking;
use crate::Node;
use fastctx_content::v2::Network;
use fastctx_net::link::Link;
use fastctx_types::access::HubId;
use std::sync::Arc;

/// The network of `hub` under its current term, if this machine is on it.
pub(crate) fn network(node: &Node, hub: &HubId) -> Option<Network> {
    node.assets.as_ref()?.networks.lock().expect("networks").get(hub).cloned()
}

/// This machine is on `hub`'s network from now on, what an earlier term
/// left held revoked first. Call holding `world::memberships`.
pub(crate) async fn joined(node: &Arc<Node>, hub: &HubId) -> Result<Option<Network>, String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(None) };
    let (theirs, device) = (hub.as_str().to_string(), assets.device());
    let network = blocking(&assets.library, move |library| library.join(&theirs, &device)).await?;
    assets.networks.lock().expect("networks").insert(hub.clone(), network.clone());
    Ok(Some(network))
}

/// This machine left `hub`'s network: every scope of it held here is
/// revoked now, its writes not yet uploaded kept; the upkeep carries on with
/// the rest. Call holding `world::memberships`.
pub(crate) async fn left(node: &Arc<Node>, hub: &HubId) {
    let Some(assets) = node.assets.as_ref() else { return };
    assets.networks.lock().expect("networks").remove(hub);
    assets.forget_listing(hub);
    let (theirs, device) = (hub.as_str().to_string(), assets.device());
    match blocking(&assets.library, move |library| library.leave(&theirs, &device)).await {
        Ok(0) => {}
        Ok(scopes) => node.log.line(format_args!("left {hub}: its {scopes} scope(s) here are let go")),
        // The library has the network still on: the upkeep finds its scopes
        // held while no membership stands and leaves it again.
        Err(why) => node.log.line(format_args!("left {hub}: letting go of its assets here: {why}; tried again")),
    }
    super::upkeep::wake(node);
}

/// The network a driver of `hub` works under over `link`: joined again
/// where the library has it off while the membership stands — a join that
/// failed — else none, the link's place ended or replaced.
pub(super) async fn of_link(node: &Arc<Node>, hub: &HubId, link: &Arc<Link>) -> Result<Option<Network>, String> {
    if let Some(network) = network(node, hub) {
        return Ok(Some(network));
    }
    let _one = crate::world::memberships().lock().await;
    let standing = node.link_of(hub).is_some_and(|current| Arc::ptr_eq(&current, link)) && link.membership_active();
    if !standing {
        return Ok(None);
    }
    joined(node, hub).await
}

/// At start, before any link: the library on the networks of the
/// memberships `kept` that stand, and off every other it has on — what a
/// stop between a membership's end and the library's left. Call on a
/// thread that may block.
pub(crate) fn settle(node: &Node, kept: &[HubId]) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let library = &assets.library;
    let device = assets.device();
    for hub in library.joined().map_err(|error| error.to_string())? {
        if !kept.iter().any(|kept| kept.as_str() == hub) {
            let scopes = library.leave(&hub, &device).map_err(|error| error.to_string())?;
            node.log.line(format_args!("{hub} was left before its {scopes} scope(s) here were let go; they are now"));
        }
    }
    for hub in kept {
        let network = library.join(hub.as_str(), &device).map_err(|error| error.to_string())?;
        assets.networks.lock().expect("networks").insert(hub.clone(), network);
    }
    Ok(())
}
