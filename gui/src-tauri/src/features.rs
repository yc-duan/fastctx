//! The interface's optional parts this device shows (`world-gui.md`, 功能),
//! and the language it speaks, remembered by the device: through the local
//! node on a computer, so the TUI shows the same, or in the device's own
//! directory on a phone.

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

/// Says the interface speaks the language tagged `language`; all the
/// switches after.
#[fastctx_gui_command::command]
pub(crate) async fn features_language(shell: tauri::State<'_, Shell>, language: String) -> Result<Features, String> {
    shell.world().set_language(&language).await.map_err(|error| error.to_string())
}

/// What a features watch sends: the switches, at first and after each
/// change, or why the watch broke while it is opened again.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    Features { features: Features },
    Interrupted { error: String, since: u64 },
}

/// Follows the switches until the page closes or `unwatch` is called with
/// the id this returns. A watch that broke, as when the node handed the
/// World on, is opened again as `Again` paces it; the switches as they are
/// then come first.
#[fastctx_gui_command::command]
pub(crate) async fn features_watch(
    shell: tauri::State<'_, Shell>,
    pieces: Channel<crate::watch_delivery::Wake>,
) -> Result<u64, String> {
    let page = shell.page.load(Ordering::Relaxed);
    let world = shell.world();
    let mut initial = Some(world.watch_features().await.map_err(|error| error.to_string())?);
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    shell.follow_delivery(
        page,
        id,
        pieces,
        |_: &Piece| Some("features".into()),
        move |pieces, _| {
            let world = world.clone();
            let initial = initial.take();
            async move {
                let mut again = fastctx_client::Again::default();
                let mut watch = if let Some(initial) = initial {
                    initial
                } else {
                    loop {
                        match world.watch_features().await {
                            Ok(watch) => break watch,
                            Err(error) => {
                                let interruption = again.broke(error.to_string());
                                if pieces
                                    .send(Piece::Interrupted {
                                        error: interruption.why,
                                        since: crate::epoch_ms(interruption.since),
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                                again.pause(&world, None).await;
                            }
                        }
                    }
                };
                loop {
                    match watch.next().await {
                        Ok(features) => {
                            again.came();
                            if pieces.send(Piece::Features { features }).is_err() {
                                return;
                            }
                            continue;
                        }
                        Err(error) => {
                            let interruption = again.broke(error.to_string());
                            let since = crate::epoch_ms(interruption.since);
                            if pieces.send(Piece::Interrupted { error: interruption.why, since }).is_err() {
                                return;
                            }
                        }
                    }
                    loop {
                        again.pause(&world, None).await;
                        match world.watch_features().await {
                            Ok(opened) => {
                                watch = opened;
                                break;
                            }
                            Err(error) => {
                                let interruption = again.broke(error.to_string());
                                let since = crate::epoch_ms(interruption.since);
                                if pieces.send(Piece::Interrupted { error: interruption.why, since }).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        },
    )?;
    Ok(id)
}
