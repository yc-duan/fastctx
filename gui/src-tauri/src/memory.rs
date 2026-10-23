//! Project memory and its account switch, on desktop and devices without a node.

use crate::Shell;
use fastctx_types::{
    AccountName, HubName, TownName,
    memory::{Catalog, Document, Kind, MemoryCall, MemoryReply, Revision, Settings},
};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::ipc::Channel;

fn town(name: String) -> Result<TownName, String> {
    TownName::try_from(name).map_err(|error| error.to_string())
}
fn scope(hub: Option<String>, account: Option<String>) -> Result<(Option<HubName>, Option<AccountName>), String> {
    Ok((
        hub.map(HubName::try_from).transpose().map_err(|error| error.to_string())?,
        account.map(AccountName::try_from).transpose().map_err(|error| error.to_string())?,
    ))
}

#[fastctx_gui_command::command]
pub(crate) async fn memory_list(shell: tauri::State<'_, Shell>, project: String) -> Result<Catalog, String> {
    match shell.world().memory(MemoryCall::List { town: town(project)? }).await.map_err(|error| error.to_string())? {
        MemoryReply::Catalog { catalog } => Ok(catalog),
        _ => Err(fastctx_types::memory::words::CATALOG_UNANSWERED.into()),
    }
}

#[fastctx_gui_command::command]
pub(crate) async fn memory_read(
    shell: tauri::State<'_, Shell>,
    project: String,
    name: String,
    version: Option<u32>,
) -> Result<Document, String> {
    match shell
        .world()
        .memory(MemoryCall::Read { town: town(project)?, name, version })
        .await
        .map_err(|error| error.to_string())?
    {
        MemoryReply::Document { document } => Ok(*document),
        _ => Err(fastctx_types::memory::words::WORLD_DOCUMENT_UNANSWERED.into()),
    }
}

#[fastctx_gui_command::command]
pub(crate) async fn memory_save_spec(
    shell: tauri::State<'_, Shell>,
    project: String,
    name: String,
    text: String,
    expected: Option<u32>,
) -> Result<Revision, String> {
    save(&shell, project, Kind::Spec, name, text, expected).await
}

#[fastctx_gui_command::command]
pub(crate) async fn memory_add_journal(
    shell: tauri::State<'_, Shell>,
    project: String,
    name: String,
    text: String,
) -> Result<Revision, String> {
    save(&shell, project, Kind::Journal, name, text, None).await
}

async fn save(
    shell: &Shell,
    project: String,
    kind: Kind,
    name: String,
    text: String,
    expected: Option<u32>,
) -> Result<Revision, String> {
    match shell
        .world()
        .memory(MemoryCall::Save { town: town(project)?, kind, name, text, expected })
        .await
        .map_err(|error| error.to_string())?
    {
        MemoryReply::Saved { revision } => Ok(*revision),
        _ => Err(fastctx_types::memory::words::SAVE_UNCONFIRMED.into()),
    }
}

#[fastctx_gui_command::command]
pub(crate) async fn memory_settings_read(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
) -> Result<Settings, String> {
    let (hub, account) = scope(hub, account)?;
    shell.world().memory_settings(hub, account, None).await.map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn memory_settings_set(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    enabled: bool,
) -> Result<Settings, String> {
    let (hub, account) = scope(hub, account)?;
    shell.world().memory_settings(hub, account, Some(enabled)).await.map_err(|error| error.to_string())
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    Catalog { catalog: Catalog },
    Interrupted { error: String, since: u64 },
}

#[fastctx_gui_command::command]
pub(crate) async fn memory_watch(
    shell: tauri::State<'_, Shell>,
    project: String,
    pieces: Channel<Piece>,
) -> Result<u64, String> {
    let town = town(project)?;
    let page = shell.page.load(Ordering::Relaxed);
    let world = shell.world();
    let mut watch = world.watch_memory(town.clone()).await.map_err(|error| error.to_string())?;
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    shell.follow(page, id, async move {
        let mut again = fastctx_client::Again::default();
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
                match world.watch_memory(town.clone()).await {
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
    })?;
    Ok(id)
}
