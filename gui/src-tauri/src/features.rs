//! The interface's optional parts this device shows (`world-gui.md`, 功能),
//! remembered by the device: through the local node on a computer, so the
//! TUI shows the same, or in the device's own directory on a phone.

use std::sync::atomic::Ordering;

use fastctx_types::features::{Feature, Features};
use serde::Serialize;
use tauri::ipc::Channel;

use crate::Shell;

#[fastctx_gui_command::command]
pub(crate) async fn features_read(shell: tauri::State<'_, Shell>) -> Result<Features, String> {
    shell.world().features().await.map_err(|error| error.to_string())
}

/// Turns one switch on or off; all of them after.
#[fastctx_gui_command::command]
pub(crate) async fn features_set(
    shell: tauri::State<'_, Shell>,
    feature: Feature,
    on: bool,
) -> Result<Features, String> {
    shell.world().set_feature(feature, on).await.map_err(|error| error.to_string())
}

/// What a features watch sends: the switches, at first and after each
/// change, or why the watch ended.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    Features { features: Features },
    Ended { error: String },
}

/// Follows the switches until the page closes or `unwatch` is called with
/// the id this returns.
#[fastctx_gui_command::command]
pub(crate) async fn features_watch(shell: tauri::State<'_, Shell>, pieces: Channel<Piece>) -> Result<u64, String> {
    let page = shell.page.load(Ordering::Relaxed);
    let mut watch = shell.world().watch_features().await.map_err(|error| error.to_string())?;
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    shell.follow(page, id, async move {
        loop {
            match watch.next().await {
                Ok(features) => {
                    if pieces.send(Piece::Features { features }).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = pieces.send(Piece::Ended { error: error.to_string() });
                    return;
                }
            }
        }
    })?;
    Ok(id)
}
