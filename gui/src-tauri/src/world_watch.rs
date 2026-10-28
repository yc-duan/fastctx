//! The World as the interface shows it (`fastctx_types::world::WorldNow`),
//! read from this computer's node or the phone's own links, asking no
//! machine; and followed as it changes, so a change elsewhere reaches the
//! page within a round trip and nothing is asked while nothing changes
//! (`world-network.md`, 近况).

use std::sync::atomic::Ordering;

use fastctx_types::world::WorldNow;
use serde::Serialize;
use tauri::ipc::Channel;

use crate::Shell;

#[fastctx_gui_command::command]
pub(crate) async fn world_now(shell: tauri::State<'_, Shell>) -> Result<WorldNow, String> {
    shell.world().world_now().await.map_err(|error| error.to_string())
}

/// What a World watch sends: the World at first, then what changed in it
/// (`crate::world_delta`), or the whole again when that is no smaller than
/// half of it; or why the watch broke while it is opened again.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    World { world: Box<WorldNow> },
    Changed { change: serde_json::Value },
    Interrupted { error: String, since: u64 },
}

/// The piece that tells `now` to a page that holds `last`: what changed,
/// or the whole; none when nothing did. `last` becomes `now`.
fn told(last: &mut Option<serde_json::Value>, now: WorldNow) -> Option<Piece> {
    let value = serde_json::to_value(&now).ok();
    let piece = match (last.as_ref(), value.as_ref()) {
        (Some(before), Some(after)) => match crate::world_delta::between(before, after) {
            None => None,
            Some(change) if length(&change) < length(after) / 2 => Some(Piece::Changed { change }),
            Some(_) => Some(Piece::World { world: Box::new(now) }),
        },
        _ => Some(Piece::World { world: Box::new(now) }),
    };
    *last = value;
    piece
}

fn length(value: &serde_json::Value) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |text| text.len())
}

/// Follows the World until the page closes or `unwatch` is called with the
/// id this returns. A watch that broke, as when the node handed the World
/// on, is opened again as `Again` paces it; the World as it is then comes
/// first.
#[fastctx_gui_command::command]
pub(crate) async fn world_watch(
    shell: tauri::State<'_, Shell>,
    pieces: Channel<crate::watch_delivery::Wake>,
) -> Result<u64, String> {
    let page = shell.page.load(Ordering::Relaxed);
    let world = shell.world();
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    shell.follow_delivery(
        page,
        id,
        pieces,
        // Each change builds on the one before: none is passed over.
        |_: &Piece| None,
        move |pieces, _| {
            let world = world.clone();
            async move {
                let mut again = fastctx_client::Again::default();
                loop {
                    match world.watch_world().await {
                        Ok(mut watch) => {
                            // A watch opened again starts from the whole.
                            let mut last = None;
                            loop {
                                match watch.next().await {
                                    Ok(now) => {
                                        again.came();
                                        if let Some(piece) = told(&mut last, now)
                                            && pieces.send(piece).is_err()
                                        {
                                            return;
                                        }
                                    }
                                    Err(error) => {
                                        let interruption = again.broke(error.to_string());
                                        let since = crate::epoch_ms(interruption.since);
                                        if pieces.send(Piece::Interrupted { error: interruption.why, since }).is_err() {
                                            return;
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            let interruption = again.broke(error.to_string());
                            let since = crate::epoch_ms(interruption.since);
                            if pieces.send(Piece::Interrupted { error: interruption.why, since }).is_err() {
                                return;
                            }
                        }
                    }
                    again.pause(&world, None).await;
                }
            }
        },
    )?;
    Ok(id)
}
