//! Plan side-panel commands. Only metadata changes are pushed; the page chooses
//! when to fetch the Markdown. The hub serves these while the host is offline.

use crate::Shell;
use fastctx_types::{
    AccountName, HubName, ResidentId,
    account_records::Metadata,
    plan::{self, Page},
};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::ipc::Channel;

type Selection = (Option<HubName>, Option<AccountName>, ResidentId);
fn named(hub: Option<String>, account: Option<String>, owner: String) -> Result<Selection, String> {
    Ok((
        hub.map(HubName::try_from).transpose().map_err(|e| e.to_string())?,
        account.map(AccountName::try_from).transpose().map_err(|e| e.to_string())?,
        owner.try_into().map_err(|e: fastctx_types::NameError| e.to_string())?,
    ))
}

#[fastctx_gui_command::command]
pub(crate) async fn plan_read(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    owner: String,
) -> Result<Page, String> {
    let (hub, account, owner) = named(hub, account, owner)?;
    shell.world().plan_read(hub, account, owner).await.map_err(|e| e.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn plan_write(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    owner: String,
    expected: Option<u64>,
    markdown: String,
) -> Result<plan::Result, String> {
    let (hub, account, owner) = named(hub, account, owner)?;
    shell.world().plan_write(hub, account, owner, expected, markdown).await.map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    Changed { metadata: Option<Metadata> },
    Interrupted { error: String, since: u64 },
}

#[fastctx_gui_command::command]
pub(crate) async fn plan_watch(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    owner: String,
    pieces: Channel<Piece>,
) -> Result<u64, String> {
    let (hub, account, owner) = named(hub, account, owner)?;
    let page = shell.page.load(Ordering::Relaxed);
    let world = shell.world();
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    shell.follow(page, id, async move {
        let mut again = fastctx_client::Again::default();
        loop {
            match world.watch_plan(hub.clone(), account.clone(), owner.clone()).await {
                Ok(mut watch) => loop {
                    match watch.next().await {
                        Ok(metadata) => {
                            again.came();
                            if pieces.send(Piece::Changed { metadata }).is_err() {
                                return;
                            }
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
                            break;
                        }
                    }
                },
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
            again.pause(&world, None).await;
        }
    })?;
    Ok(id)
}
