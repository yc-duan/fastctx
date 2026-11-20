//! What ended holding periods and left networks leave, carried on until it
//! is done (design §4.3, §4.5): a scope of a network this machine is not on
//! that is still held is let go, every purge pass is run to its end, and
//! objects nothing holds any more are collected once their grace is over.
//! It works only from what the library records, so a stop midway goes on
//! at the next start. It runs at start, whenever something ends, and every
//! few minutes.

use super::session::blocking;
use crate::Node;
use fastctx_types::access::HubId;
use std::sync::Arc;
use std::time::Duration;

/// How often the upkeep looks when nothing woke it.
const LOOK_AGAIN: Duration = Duration::from_secs(300);
/// The pause between two purge or collection passes: other writes go
/// between them.
const PACE: Duration = Duration::from_millis(20);

/// Starts the upkeep.
pub(crate) fn start(node: &Arc<Node>) {
    if node.assets.is_none() {
        return;
    }
    let node = Arc::downgrade(node);
    fastctx_platform::task::spawn(async move { run(node).await });
}

/// Brings the upkeep round.
pub(crate) fn wake(node: &Node) {
    if let Some(assets) = node.assets.as_ref() {
        assets.upkeep.notify_one();
    }
}

async fn run(node: std::sync::Weak<Node>) {
    let _doing = fastctx_platform::doing::doing("carrying on the asset library's upkeep");
    let mut previous: Option<String> = None;
    loop {
        let Some(node) = node.upgrade() else { return };
        let Some(woken) = node.assets.as_ref().map(|assets| assets.upkeep.clone()) else { return };
        match once(&node).await {
            Err(why) if previous.as_ref() != Some(&why) => {
                node.log.line(format_args!("the asset library's upkeep: {why}; tried again"));
                previous = Some(why);
            }
            Err(_) => {}
            Ok(()) => previous = None,
        }
        drop(node);
        tokio::select! {
            () = tokio::time::sleep(LOOK_AGAIN) => {},
            () = woken.notified() => {},
        }
    }
}

async fn once(node: &Arc<Node>) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let library = &assets.library;
    // The networks the library has on, or has scopes held of, that no
    // membership here stands for: a leaving that failed or stopped midway.
    let mut off: Vec<String> = blocking(library, |library| library.joined()).await?;
    off.extend(blocking(library, |library| library.left_held()).await?);
    off.sort();
    off.dedup();
    let stands = |hub: &HubId| node.link_of(hub).is_some_and(|link| link.membership_active());
    for hub in off {
        let Ok(hub) = HubId::try_from(hub) else { continue };
        if stands(&hub) {
            continue;
        }
        // Judged again under the membership lock: a membership begun
        // between the look and now stands.
        let _one = crate::world::memberships().lock().await;
        if !stands(&hub) {
            super::membership::left(node, &hub, node.link_of(&hub).as_deref()).await;
        }
    }
    while blocking(library, |library| library.purging()).await? {
        blocking(library, |library| library.purge()).await?;
        tokio::time::sleep(PACE).await;
    }
    while blocking(library, |library| library.collect()).await?.objects > 0 {
        tokio::time::sleep(PACE).await;
    }
    Ok(())
}
