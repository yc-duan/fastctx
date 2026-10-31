//! Project and conversation history, including direct hub access on phones.

use crate::Shell;
use fastctx_types::history::{Catalog, Document, HistoryCall, HistoryReply, HistoryRevision, Scope};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::ipc::Channel;

#[fastctx_gui_command::command]
pub(crate) async fn history_list(shell: tauri::State<'_, Shell>, scope: Scope) -> Result<Catalog, String> {
    match shell.world().history(HistoryCall::List { scope }).await.map_err(|error| error.to_string())? {
        HistoryReply::Catalog { catalog } => Ok(catalog),
        _ => Err("the history catalog was not returned".into()),
    }
}

#[fastctx_gui_command::command]
pub(crate) async fn history_read(
    shell: tauri::State<'_, Shell>,
    scope: Scope,
    name: String,
    version: Option<u64>,
) -> Result<Document, String> {
    match shell.world().history(HistoryCall::Read { scope, name, version }).await.map_err(|error| error.to_string())? {
        HistoryReply::Document { document } => Ok(*document),
        _ => Err("the history document was not returned".into()),
    }
}

#[fastctx_gui_command::command]
pub(crate) async fn history_save(
    shell: tauri::State<'_, Shell>,
    scope: Scope,
    name: String,
    text: String,
    expected: Option<u64>,
) -> Result<HistoryRevision, String> {
    saved(&shell, HistoryCall::Save { scope, name, text, expected }).await
}

#[fastctx_gui_command::command]
pub(crate) async fn history_retire(
    shell: tauri::State<'_, Shell>,
    scope: Scope,
    name: String,
    expected: u64,
) -> Result<HistoryRevision, String> {
    saved(&shell, HistoryCall::Retire { scope, name, expected }).await
}

#[fastctx_gui_command::command]
pub(crate) async fn history_restore(
    shell: tauri::State<'_, Shell>,
    scope: Scope,
    name: String,
    version: u64,
) -> Result<HistoryRevision, String> {
    saved(&shell, HistoryCall::Restore { scope, name, version }).await
}

async fn saved(shell: &Shell, request: HistoryCall) -> Result<HistoryRevision, String> {
    match shell.world().history(request).await.map_err(|error| error.to_string())? {
        HistoryReply::Saved { revision } => Ok(*revision),
        _ => Err("the history change was not confirmed".into()),
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    Catalog { catalog: Catalog },
    Interrupted { error: String, since: u64 },
}

#[fastctx_gui_command::command]
pub(crate) async fn history_watch(
    shell: tauri::State<'_, Shell>,
    scope: Scope,
    pieces: Channel<crate::watch_delivery::Wake>,
) -> Result<u64, String> {
    let page = shell.page.load(Ordering::Relaxed);
    let world = shell.world();
    let mut initial = Some(world.watch_history(scope.clone()).await.map_err(|error| error.to_string())?);
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    shell.follow_delivery(
        page,
        id,
        pieces,
        |_: &Piece| Some("catalog".into()),
        move |pieces, _| {
            let (world, scope) = (world.clone(), scope.clone());
            let initial = initial.take();
            async move {
                let mut again = fastctx_client::Again::default();
                let mut watch = if let Some(initial) = initial {
                    initial
                } else {
                    loop {
                        match world.watch_history(scope.clone()).await {
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
                        Ok(catalog) => {
                            again.came();
                            if pieces.send(Piece::Catalog { catalog }).is_err() {
                                return;
                            }
                            continue;
                        }
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
                        }
                    }
                    loop {
                        again.pause(&world, None).await;
                        match world.watch_history(scope.clone()).await {
                            Ok(opened) => {
                                watch = opened;
                                break;
                            }
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
                            }
                        }
                    }
                }
            }
        },
    )?;
    Ok(id)
}
