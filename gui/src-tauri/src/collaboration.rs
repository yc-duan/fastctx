//! The conversation side panel's collaboration tree: read once, or followed
//! while the panel shows it.

use crate::Shell;
use fastctx_client::{CollaborationWatch, World};
use fastctx_types::{ResidentId, access::Written, collaboration::Tree};
use serde::Serialize;
use tauri::ipc::Channel;

#[fastctx_gui_command::command]
pub(crate) async fn collaboration_tree(
    shell: tauri::State<'_, Shell>,
    resident: String,
    machine: Option<String>,
) -> Result<Tree, String> {
    let resident = ResidentId::try_from(resident).map_err(|error| error.to_string())?;
    let machine = machine.map(|machine| Written::parse(&machine)).transpose().map_err(|error| error.to_string())?;
    shell.world().collaboration(resident, machine).await.map_err(|error| error.to_string())
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    Tree { tree: Box<Tree> },
    Interrupted { error: String, since: u64 },
}

#[derive(Serialize)]
pub struct Watching {
    id: u64,
    tree: Tree,
}

/// Follows a conversation's collaboration: the tree now, then each time it
/// changes, opened again after a break.
#[fastctx_gui_command::command]
pub(crate) async fn collaboration_watch(
    shell: tauri::State<'_, Shell>,
    resident: String,
    machine: Option<String>,
    pieces: Channel<crate::watch_delivery::Wake>,
) -> Result<Watching, String> {
    let page = shell.page.load(std::sync::atomic::Ordering::Relaxed);
    let world = shell.world();
    let resident = ResidentId::try_from(resident).map_err(|error| error.to_string())?;
    let machine = machine.map(|machine| Written::parse(&machine)).transpose().map_err(|error| error.to_string())?;
    #[cfg(target_os = "android")]
    crate::android::readiness::wait(&world, machine.as_ref()).await?;
    let (watch, tree) =
        world.watch_collaboration(resident.clone(), machine.clone()).await.map_err(|error| error.to_string())?;
    let id = shell.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut initial = Some(watch);
    shell.follow_delivery(
        page,
        id,
        pieces,
        |_: &Piece| Some("collaboration".into()),
        move |pieces, _| {
            let (world, resident, machine) = (world.clone(), resident.clone(), machine.clone());
            let initial = initial.take();
            async move {
                let mut again = fastctx_client::Again::default();
                let mut watch = match initial {
                    Some(watch) => watch,
                    None => match open(&world, &resident, &machine, &mut again, &pieces).await {
                        Some(watch) => watch,
                        None => return,
                    },
                };
                again.came();
                loop {
                    let ended = match watch.next().await {
                        Ok(Some(tree)) => {
                            if pieces.send(Piece::Tree { tree: Box::new(tree) }).is_err() {
                                return;
                            }
                            continue;
                        }
                        Ok(None) => (fastctx_types::collab_prompt::WATCH_ENDED.to_string(), false),
                        Err(error) => (error.to_string(), error.is_deleted() || error.is_refused()),
                    };
                    let interruption = again.broke(ended.0);
                    let interrupted =
                        Piece::Interrupted { error: interruption.why, since: crate::epoch_ms(interruption.since) };
                    if pieces.send(interrupted).is_err() || ended.1 {
                        return;
                    }
                    again.pause(&world, machine.as_ref()).await;
                    watch = match open(&world, &resident, &machine, &mut again, &pieces).await {
                        Some(watch) => watch,
                        None => return,
                    };
                    again.came();
                }
            }
        },
    )?;
    Ok(Watching { id, tree })
}

/// Opens the watch again, telling the page the tree it opens with and each
/// break on the way; `None` once the page is gone or the conversation is
/// out of reach for good.
async fn open(
    world: &World,
    resident: &ResidentId,
    machine: &Option<Written>,
    again: &mut fastctx_client::Again,
    pieces: &crate::watch_delivery::Sender<Piece>,
) -> Option<CollaborationWatch> {
    loop {
        match world.watch_collaboration(resident.clone(), machine.clone()).await {
            Ok((watch, tree)) => return pieces.send(Piece::Tree { tree: Box::new(tree) }).ok().map(|()| watch),
            Err(error) => {
                let lost = error.is_deleted() || error.is_refused();
                let interruption = again.broke(error.to_string());
                let interrupted =
                    Piece::Interrupted { error: interruption.why, since: crate::epoch_ms(interruption.since) };
                if pieces.send(interrupted).is_err() || lost {
                    return None;
                }
                again.pause(world, machine.as_ref()).await;
            }
        }
    }
}
