//! A private notes setting while alone, carried into the first registered account.

use crate::Node;
use fastctx_net::{ids::HubId, link::Link};
use fastctx_types::{history_switch::HistorySwitch, ipc::Failure, layout::WorldDir};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Default)]
pub(crate) struct Keeping {
    value: Mutex<Option<HistorySwitch>>,
    generation: AtomicU64,
    carrying: crate::ranked::Carrying,
}

fn path(world: &WorldDir) -> PathBuf {
    world.path().join("history-switch.json")
}

fn read_at(path: &Path) -> std::io::Result<Option<HistorySwitch>> {
    match fastctx_platform::disk::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(std::io::Error::other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn keep(world: &WorldDir, setting: HistorySwitch) -> std::io::Result<()> {
    fastctx_platform::publish(
        &path(world),
        &serde_json::to_vec(&setting).expect("switch serializes"),
        fastctx_platform::Permissions::Private,
    )
}

pub(crate) fn read(node: &Node) -> Result<HistorySwitch, Failure> {
    let mut value = node.own_history.value.lock().expect("never poisoned");
    if let Some(setting) = *value {
        return Ok(setting);
    }
    let setting = read_at(&path(&node.world))
        .map_err(|error| Failure::new(format!("reading the notes setting: {error}")))?
        .unwrap_or_default();
    *value = Some(setting);
    Ok(setting)
}

pub(crate) fn write(node: &Node, on: bool) -> Result<(), Failure> {
    let mut value = node.own_history.value.lock().expect("never poisoned");
    if node.connected_anywhere() {
        return Err(Failure::not_run(
            "this computer is in a network; name the account whose notes setting is being changed",
        ));
    }
    let setting = HistorySwitch { on };
    keep(&node.world, setting).map_err(|error| Failure::new(format!("writing the notes setting: {error}")))?;
    *value = Some(setting);
    node.own_history.generation.fetch_add(1, Ordering::Relaxed);
    drop(value);
    node.views_changed();
    node.shown_changed();
    Ok(())
}

/// Retry on reconnect until the account confirms the carried choice.
pub(crate) async fn carry(node: &Arc<Node>, hub: &HubId) {
    if crate::managed::is_managed(node) {
        return;
    }
    let _carrying = node.own_history.carrying.lock().await;
    if let Err(error) = carry_now(node, hub).await {
        node.log.line(format_args!("carrying this computer's notes setting: {error}"));
    }
}

async fn carry_now(node: &Arc<Node>, hub: &HubId) -> Result<(), String> {
    let own_path = path(&node.world);
    let (setting, generation) = {
        let _value = node.own_history.value.lock().expect("never poisoned");
        let Some(setting) = read_at(&own_path).map_err(|error| error.to_string())? else { return Ok(()) };
        (setting, node.own_history.generation.load(Ordering::Relaxed))
    };
    let Some(link) = node.link_of(hub) else { return Ok(()) };
    link.refresh().await?;
    let view = link.view();
    let Some(home) = view.me().and_then(|me| me.home.clone()) else { return Ok(()) };
    if !view.history_switch || !view.logged_in(&view.me, &home) {
        return Ok(());
    }
    if setting.on && !link.history_switch_read(&home)?.on {
        link.history_switch_write(&home, true).await?;
    }
    let mut value = node.own_history.value.lock().expect("never poisoned");
    // Leaving or writing while the hub was answering keeps the newer local choice.
    if node.own_history.generation.load(Ordering::Relaxed) != generation
        || !node.link_of(hub).is_some_and(|current| Arc::ptr_eq(&current, &link))
    {
        return Ok(());
    }
    fastctx_platform::disk::rename(&own_path, node.world.path().join("history-switch.carried.json"))
        .map_err(|error| format!("the account kept the switch, but putting its local copy aside failed: {error}"))?;
    *value = None;
    Ok(())
}

/// The last network's registered account becomes the computer's own choice.
pub(crate) fn take_back(node: &Node, link: &Link) {
    let mut value = node.own_history.value.lock().expect("never poisoned");
    node.own_history.generation.fetch_add(1, Ordering::Relaxed);
    let result = (|| -> std::io::Result<()> {
        if read_at(&path(&node.world))?.is_some() {
            return Ok(());
        }
        let view = link.view();
        let Some(account) = view.me().and_then(|me| me.home.as_ref()).and_then(|home| view.account(home)) else {
            return Ok(());
        };
        let setting = HistorySwitch { on: account.history };
        keep(&node.world, setting)?;
        *value = Some(setting);
        Ok(())
    })();
    if let Err(error) = result {
        node.log.line(format_args!("taking this computer's notes setting back: {error}"));
    }
}
