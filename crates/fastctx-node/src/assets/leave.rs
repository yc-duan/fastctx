//! What a network this machine leaves had here goes with it (the user,
//! 2026-11-12): every scope its hub described is revoked as the hub revoking
//! it would revoke it — this machine's writes not yet uploaded kept aside —
//! every scope this machine began for that network goes too, and what ended
//! is purged. Leaving is written down before it begins (`world::log_out`),
//! so a node stopped midway leaves again when it starts, and this runs again.

use super::Proposal;
use super::session::blocking;
use crate::Node;
use fastctx_types::access::HubId;
use fastctx_types::assets as wire;
use std::sync::Arc;
use std::time::Duration;

/// How long leaving waits for the network's driver to end its pass: one
/// still running after that may describe nothing new, its link gone.
const DRIVER_ENDS: Duration = Duration::from_secs(30);
/// Purge passes at most while leaving; what is left goes at a later pass of
/// another network's driver, or the next leaving.
const PURGES: usize = 100_000;

/// Clears what `hub`'s network had here. Its link is gone already.
pub(crate) async fn leave(node: &Arc<Node>, hub: &HubId) {
    let Some(assets) = node.assets.as_ref() else { return };
    // Its driver ends at its next round, the link gone: what a pass under
    // way installs is cleared with the rest once it has.
    let began = std::time::Instant::now();
    loop {
        let wake = assets.drivers.lock().expect("drivers").get(hub).cloned();
        let Some(wake) = wake else { break };
        if began.elapsed() > DRIVER_ENDS {
            node.log.line(format_args!("leaving {hub}: its asset driver has not ended; its assets are cleared all the same"));
            break;
        }
        wake.notify_one();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assets.forget_listing(hub);
    match clear(node, hub).await {
        Ok(0) => {}
        Ok(scopes) => node.log.line(format_args!("left {hub}: its {scopes} scope(s) here are cleared")),
        Err(why) => node.log.line(format_args!("left {hub}: clearing its assets here: {why}")),
    }
}

/// At start, once every network this computer is on has its link: what a
/// network it is on no more had here goes, as leaving it would clear it.
pub(crate) async fn left_before(node: Arc<Node>) {
    let Some(assets) = node.assets.as_ref() else { return };
    let library = &assets.library;
    let on: std::collections::BTreeSet<String> = node.links().iter().map(|link| link.membership().id.as_str().to_string()).collect();
    let mut gone: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    match blocking(library, |library| library.described_scopes()).await {
        Ok(described) => gone.extend(
            described
                .iter()
                .filter_map(|(_, descriptor)| serde_json::from_slice::<wire::Descriptor>(descriptor).ok())
                .map(|descriptor| descriptor.hub)
                .filter(|hub| !on.contains(hub)),
        ),
        Err(why) => node.log.line(format_args!("finding what networks left had here: {why}")),
    }
    match blocking(library, |library| library.provisionals()).await {
        Ok(begun) => gone.extend(
            begun
                .iter()
                .filter_map(|(_, proposal)| proposal.as_deref().and_then(|proposal| serde_json::from_slice::<Proposal>(proposal).ok()))
                .map(|proposal| proposal.hub)
                .filter(|hub| !on.contains(hub)),
        ),
        Err(why) => node.log.line(format_args!("finding what networks left had here: {why}")),
    }
    for hub in gone {
        let Ok(hub) = HubId::try_from(hub) else { continue };
        match clear(&node, &hub).await {
            Ok(0) => {}
            Ok(scopes) => node.log.line(format_args!("{hub} was left before its {scopes} scope(s) here were cleared; they are now")),
            Err(why) => node.log.line(format_args!("clearing what {hub}, left, had here: {why}")),
        }
    }
}

/// Revokes every scope of `hub`'s network still held here and purges what
/// ended: how many scopes were held.
async fn clear(node: &Arc<Node>, hub: &HubId) -> Result<usize, String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(0) };
    let library = &assets.library;
    let theirs = hub.as_str().to_string();
    let mut scopes: Vec<String> = blocking(library, |library| library.described_scopes())
        .await?
        .into_iter()
        .filter(|(_, descriptor)| serde_json::from_slice::<wire::Descriptor>(descriptor).is_ok_and(|descriptor| descriptor.hub == theirs))
        .map(|(scope, _)| scope)
        .collect();
    scopes.extend(
        blocking(library, |library| library.provisionals())
            .await?
            .into_iter()
            .filter(|(_, proposal)| {
                proposal.as_deref().and_then(|proposal| serde_json::from_slice::<Proposal>(proposal).ok()).is_some_and(|proposal| proposal.hub == theirs)
            })
            .map(|(scope, _)| scope),
    );
    let device = assets.device();
    let mut held = 0;
    for scope in &scopes {
        let key = scope.clone();
        if blocking(library, move |library| library.holding(&key)).await?.is_none() {
            continue;
        }
        held += 1;
        let key = scope.clone();
        blocking(library, move |library| library.revoke_keeping(&key, &device)).await?;
    }
    for _ in 0..PURGES {
        if !blocking(library, |library| library.purging()).await? {
            break;
        }
        blocking(library, |library| library.purge()).await?;
    }
    Ok(held)
}
