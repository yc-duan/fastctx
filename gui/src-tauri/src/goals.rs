//! Goal state is authoritative at its account; the shell keeps only a live subscription.

use crate::{Shell, machine, resident};
use fastctx_client::{World, goals::GoalWatch};
use fastctx_types::ResidentId;
use fastctx_types::access::{HubAccount, Written};
use fastctx_types::goals::{Request, Updated, UserChange, View};
use serde::Serialize;
use tauri::ipc::Channel;

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    Snapshot { view: Box<View> },
    Interrupted { error: String, since: u64 },
}

#[derive(Serialize)]
pub struct Watching {
    id: u64,
    view: View,
}

#[fastctx_gui_command::command]
pub async fn goal_read(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    account: Option<HubAccount>,
) -> Result<View, String> {
    let updated = shell
        .world()
        .goal(resident(resident_id)?, machine(machine_name)?, account, Request::Read)
        .await
        .map_err(|error| error.to_string())?;
    match updated {
        Updated::Goal { view } => Ok(*view),
        Updated::Conflict { .. } | Updated::History { .. } => Err(fastctx_types::goals::words::INVALID_READ.into()),
    }
}

#[fastctx_gui_command::command]
#[allow(clippy::too_many_arguments)]
pub async fn goal_update(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    account: Option<HubAccount>,
    operation_id: String,
    expected: Option<u64>,
    change: UserChange,
) -> Result<Updated, crate::NotSaid> {
    let refused = |message| crate::NotSaid { message, again: false };
    shell
        .world()
        .goal(
            resident(resident_id).map_err(refused)?,
            machine(machine_name).map_err(refused)?,
            account,
            Request::Change { operation: operation_id, expected, change },
        )
        .await
        .map_err(|error| crate::NotSaid { again: World::unanswered(&error), message: error.to_string() })
}

#[fastctx_gui_command::command]
pub async fn goal_watch(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    account: Option<HubAccount>,
    pieces: Channel<Piece>,
) -> Result<Watching, String> {
    let page = shell.page.load(std::sync::atomic::Ordering::Relaxed);
    let world = shell.world();
    let resident = resident(resident_id)?;
    let machine = machine(machine_name)?;
    #[cfg(target_os = "android")]
    crate::android::readiness::wait(&world, machine.as_ref()).await?;
    let (watch, view) =
        world.watch_goal(resident.clone(), machine.clone(), account).await.map_err(|error| error.to_string())?;
    let id = shell.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    shell.follow(page, id, follow(world, resident, machine, view.account.clone(), watch, pieces))?;
    Ok(Watching { id, view })
}

async fn follow(
    world: World,
    resident: ResidentId,
    machine: Option<Written>,
    account: HubAccount,
    mut watch: GoalWatch,
    pieces: Channel<Piece>,
) {
    let mut again = fastctx_client::Again::default();
    again.came();
    loop {
        let ended = match watch.next().await {
            Ok(Some(view)) => {
                if pieces.send(Piece::Snapshot { view: Box::new(view) }).is_err() {
                    return;
                }
                continue;
            }
            Ok(None) => (fastctx_types::goals::words::WATCH_ENDED.into(), false),
            Err(error) => (error.to_string(), error.is_deleted()),
        };
        let interruption = again.broke(ended.0);
        if pieces
            .send(Piece::Interrupted { error: interruption.why, since: crate::epoch_ms(interruption.since) })
            .is_err()
            || ended.1
        {
            return;
        }
        loop {
            again.pause(&world, machine.as_ref()).await;
            match world.watch_goal(resident.clone(), machine.clone(), Some(account.clone())).await {
                Ok((fresh, view)) => {
                    watch = fresh;
                    again.came();
                    if pieces.send(Piece::Snapshot { view: Box::new(view) }).is_err() {
                        return;
                    }
                    break;
                }
                Err(error) => {
                    let deleted = error.is_deleted();
                    let interruption = again.broke(error.to_string());
                    if pieces
                        .send(Piece::Interrupted {
                            error: interruption.why,
                            since: crate::epoch_ms(interruption.since),
                        })
                        .is_err()
                        || deleted
                    {
                        return;
                    }
                }
            }
        }
    }
}

#[fastctx_gui_command::command]
pub async fn goal_history(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    account: Option<HubAccount>,
    page: Option<String>,
) -> Result<Option<Box<fastctx_types::goals::HistoryPage>>, String> {
    match shell
        .world()
        .goal(resident(resident_id)?, machine(machine_name)?, account, Request::History { page })
        .await
        .map_err(|error| error.to_string())?
    {
        Updated::History { page } => Ok(page),
        _ => Err(fastctx_types::goals::words::INVALID_READ.into()),
    }
}
