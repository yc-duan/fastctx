//! The World as the interface shows it (`fastctx_types::world::WorldNow`),
//! read from this computer's node or the phone's own links, asking no
//! machine; and followed as it changes, so a change elsewhere reaches the
//! page within a round trip and nothing is asked while nothing changes
//! (`world-network.md`, 近况).

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use fastctx_types::world::WorldNow;
use serde::Serialize;
use tauri::ipc::Channel;

use crate::Shell;

#[fastctx_gui_command::command]
pub(crate) async fn world_now(shell: tauri::State<'_, Shell>, known: Option<Vec<String>>) -> Result<WorldNow, String> {
    shell.world().world_now_known(known.unwrap_or_default()).await.map_err(|error| error.to_string())
}

/// What a World watch sends: the World at first, then what changed in it
/// (`fastctx_types::world_delta`), or the whole again when that is no smaller than
/// half of it; or why the watch broke while it is opened again.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    World { world: Box<WorldNow> },
    Changed { change: serde_json::Value },
    Interrupted { error: String, since: u64 },
}

/// The piece that tells what the watch saw to a page that holds `last`:
/// what changed, or the whole; none when nothing did. A change the node
/// told is passed on as it is; a whole World, measured against `last`.
/// `last` becomes what the page holds then.
fn told(last: &mut Option<serde_json::Value>, seen: fastctx_client::world_now::Seen) -> Option<Piece> {
    use fastctx_types::world_delta::{Told, apply, tell};
    match seen {
        fastctx_client::world_now::Seen::Changed(change) => {
            if let Some(held) = last.as_mut() {
                apply(held, &change);
            }
            Some(Piece::Changed { change })
        }
        fastctx_client::world_now::Seen::World(now) => {
            let value = serde_json::to_value(&now).ok()?;
            match tell(last, value)? {
                Told::Changed(change) => Some(Piece::Changed { change }),
                Told::Whole(_) => Some(Piece::World { world: now }),
            }
        }
    }
}

/// Follows the World until the page closes or `unwatch` is called with the
/// id this returns. A watch that broke, as when the node handed the World
/// on, is opened again as `Again` paces it; the World as it is then comes
/// first.
#[fastctx_gui_command::command]
pub(crate) async fn world_watch(
    shell: tauri::State<'_, Shell>,
    pieces: Channel<crate::watch_delivery::Wake>,
    known: Option<Vec<String>>,
) -> Result<u64, String> {
    let page = shell.page.load(Ordering::Relaxed);
    let known = known.unwrap_or_default();
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
            let known = known.clone();
            async move {
                let mut known: BTreeSet<_> = known.into_iter().collect();
                let mut again = fastctx_client::Again::default();
                loop {
                    match world.watch_world_known(known.iter().cloned().collect()).await {
                        Ok(mut watch) => {
                            // A watch opened again starts from the whole.
                            let mut last = None;
                            loop {
                                match watch.next_seen().await {
                                    Ok(seen) => {
                                        again.came();
                                        let piece = told(&mut last, seen);
                                        if let Some(machines) =
                                            last.as_ref().and_then(|world| world["who"]["machines"].as_array())
                                        {
                                            for machine in machines {
                                                if let Some(name) = machine["reply"]["machine"]
                                                    .as_str()
                                                    .or_else(|| machine["machine"].as_str())
                                                {
                                                    known.insert(name.to_owned());
                                                }
                                            }
                                        }
                                        if let Some(piece) = piece
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
