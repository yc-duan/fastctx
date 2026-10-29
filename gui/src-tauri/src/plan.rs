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

/// The conversation's machine, which tells its plan from another's of the
/// same short name.
fn machine_named(machine: Option<String>) -> Result<Option<fastctx_types::MachineName>, String> {
    machine.map(fastctx_types::MachineName::try_from).transpose().map_err(|e| e.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn plan_read(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    owner: String,
    machine: Option<String>,
) -> Result<Page, String> {
    let (hub, account, owner) = named(hub, account, owner)?;
    shell.world().plan_read(hub, account, owner, machine_named(machine)?).await.map_err(|e| e.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn plan_write(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    owner: String,
    expected: Option<u64>,
    markdown: String,
    machine: Option<String>,
) -> Result<plan::Result, String> {
    let (hub, account, owner) = named(hub, account, owner)?;
    shell
        .world()
        .plan_write(hub, account, owner, expected, markdown, machine_named(machine)?)
        .await
        .map_err(|e| e.to_string())
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
    pieces: Channel<crate::watch_delivery::Wake>,
) -> Result<u64, String> {
    let (hub, account, owner) = named(hub, account, owner)?;
    let page = shell.page.load(Ordering::Relaxed);
    let world = shell.world();
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    shell.follow_delivery(
        page,
        id,
        pieces,
        |_: &Piece| Some("plan".into()),
        move |pieces, _| {
            let (world, hub, account, owner) = (world.clone(), hub.clone(), account.clone(), owner.clone());
            async move {
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
            }
        },
    )?;
    Ok(id)
}
