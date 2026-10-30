//! The hub of a dissolved network (`world-network.md`, 解散网络). It lets
//! nobody in again, and tells each machine and device that was on the
//! network, as each comes, that the network is gone: one away then learns it
//! once it is back. When every one was told, the hub retires as a hub that
//! moved away does: its data here is deleted, all but its log, and its
//! system service removed.

use std::path::Path;
use std::time::Duration;

use fastctx_net::sign::PublicKey;
use fastctx_net::wire::Refusal;

use crate::Hub;
use crate::store::Store;

/// How long the hub still answers after the last one was told, for a
/// machine whose answer was lost on its way and that asks again.
const GRACE: Duration = Duration::from_secs(10 * 60);

/// Whether `refusal` tells a machine that its network was dissolved.
pub(crate) fn tells(refusal: &Refusal) -> bool {
    matches!(refusal, Refusal::Removed { dissolved: true, .. })
}

/// The machine or device holding `key` was told the network is dissolved.
pub(crate) fn told(hub: &Hub, key: &PublicKey) {
    let mut store = hub.store();
    let Some(dissolved) = store.hub.dissolved.as_mut() else { return };
    let before = dissolved.untold.len();
    dissolved.untold.retain(|untold| untold != key);
    let left = dissolved.untold.len();
    if left == before {
        return;
    }
    if let Err(error) = store.save_hub() {
        hub.log.line(format_args!("keeping who was told the network is dissolved: {error}"));
    }
    drop(store);
    hub.log.line(format_args!("a machine was told its network is dissolved; {left} not told yet"));
    changed(hub);
}

/// Who is still to be told may have changed, or the network was dissolved.
pub(crate) fn changed(hub: &Hub) {
    hub.dissolution.notify_one();
}

/// Resolves once the network is dissolved, every machine and device on it
/// was told, and the grace after passed; with why the hub retires.
pub(crate) async fn settled(hub: &Hub) -> String {
    loop {
        let changed = hub.dissolution.notified();
        let left = hub.store().hub.dissolved.as_ref().map(|dissolved| dissolved.untold.len());
        if left == Some(0) {
            break;
        }
        changed.await;
    }
    hub.log.line(format_args!(
        "every machine on the dissolved network was told; the hub retires in {} min",
        GRACE.as_secs() / 60
    ));
    tokio::time::sleep(GRACE).await;
    format!("the network {} was dissolved, and every machine on it was told", hub.store().hub.name)
}

/// Deletes the data of the dissolved hub kept in `dir`, all but its log, as
/// its administrator asks before every machine was told; nothing may run
/// the hub then. How many were not told.
pub fn retire(dir: &Path) -> anyhow::Result<usize> {
    let hub = Store::read_hub(dir)?;
    let Some(dissolved) = hub.dissolved else {
        anyhow::bail!("the network {} kept in {} is not dissolved", hub.name, dir.display());
    };
    let untold = dissolved.untold.len();
    let why = format!("the network {} was dissolved; its hub was ended with {untold} machines not told", hub.name);
    crate::moving::clear(dir, &why)?;
    Ok(untold)
}

/// The network kept in `dir` and how many are still to be told, when it is
/// a dissolved one.
pub fn dissolved(dir: &Path) -> anyhow::Result<Option<(fastctx_types::HubName, usize)>> {
    if !Store::hub_file(dir).try_exists()? && !dir.join(crate::store::shelf::FILE).try_exists()? {
        return Ok(None);
    }
    let hub = Store::read_hub(dir)?;
    Ok(hub.dissolved.map(|dissolved| (hub.name, dissolved.untold.len())))
}
