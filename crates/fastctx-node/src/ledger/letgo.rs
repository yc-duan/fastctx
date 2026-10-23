//! Reconciles each conversation's project access before deciding whether
//! this machine still needs a ledger for another account.

use std::io;
use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::TownName;
use fastctx_types::management::Management;

use crate::Node;
use fastctx_types::management::ManagementOutcome;

/// Lets go of every project of `link`'s hub this computer may no longer
/// keep, as its view now says.
pub async fn let_go(node: &Arc<Node>, link: &Arc<Link>) {
    if let Err(error) = super::deletion::finish(node, link).await {
        node.log.line(format_args!("finishing project deletions: {error}"));
        return;
    }
    let hub = link.membership().id.clone();
    if link.view().me().is_none() {
        return;
    }
    let towns = match node.world.towns() {
        Ok(towns) => towns,
        Err(error) => return node.log.line(format_args!("listing the projects kept here: {error}")),
    };
    let mut gone = false;
    for town in towns {
        let place = match super::names::place(&node.world, &town) {
            Ok(Some(place)) if place.hub == hub => place,
            Ok(_) => continue,
            Err(error) => {
                node.log.line(format_args!("reading the project {town}: {error}"));
                continue;
            }
        };
        let kept = {
            let view = link.view();
            let Some(me) = view.me().map(|me| me.name.clone()) else { return };
            view.member_keeps_project(&me, &place.town)
        };
        let living: Vec<_> =
            node.residents.everyone().filter(|resident| resident.town.as_ref() == Some(&town)).collect();
        let mut stuck = false;
        for resident in living {
            let id = &resident.id;
            match crate::town::access::permits(node, &resident.about.acting, &town) {
                Ok(true) => continue,
                Err(error) => {
                    node.log.line(format_args!("checking {id}'s project access: {error}"));
                    stuck = true;
                    continue;
                }
                Ok(false) => {}
            }
            let moved = if resident.removed.is_some() || resident.about.session.is_none() {
                leave_kept(node, id).await
            } else {
                crate::management::here(node, id, Management::Move { town: None }).await.and_then(|result| {
                    if result.town.is_some()
                        || result.steps.iter().any(|step| step.outcome == ManagementOutcome::Failed)
                    {
                        Err(fastctx_types::ipc::Failure::new(format!(
                            "project removal was incomplete: {:?}",
                            result.steps
                        )))
                    } else {
                        Ok(())
                    }
                })
            };
            if let Err(failure) = moved {
                node.log.line(format_args!("moving {id} out of {town}: {failure}"));
                stuck = true;
            }
        }
        if kept {
            continue;
        }
        let used = link.view().member_in_project(&link.view().me, &place.town);
        let removed = if used || stuck { remove_copy(node, &town).await } else { remove(node, &town).await };
        match removed {
            Ok(()) => {
                node.log.line(format_args!("let go of the project {town}: no account here may keep it"));
                gone = true;
            }
            Err(error) => node.log.line(format_args!("letting go of the project {town}: {error}")),
        }
    }
    if gone {
        node.shown_changed();
    }
}

/// Archived or not-yet-paired conversations also lose their project record.
pub(crate) async fn leave_kept(
    node: &Arc<Node>,
    id: &fastctx_types::ResidentId,
) -> Result<(), fastctx_types::ipc::Failure> {
    let gate = node.residents.management(id)?;
    let _exclusive = crate::gate_waits::alone(node, id, &gate).await?;
    node.residents
        .relocate_kept(&node.world, id, None)
        .map_err(|error| fastctx_types::ipc::Failure::new(error.to_string()))?;
    crate::town::instructions::hand_to_current(node, id)
        .map_err(|error| fastctx_types::ipc::Failure::new(error.to_string()))?;
    crate::codex::moved(node, id).await;
    Ok(())
}

/// Removes what this computer kept of `town`: its ledger, its signed
/// entries, attachments, pages and records; the folder itself only when
/// nothing else is left in it, as a conversation may stand there.
pub(super) async fn remove_copy(node: &Arc<Node>, town: &TownName) -> io::Result<()> {
    let _syncing = node.syncing.lock().await;
    let directory = node.world.town(town);
    tokio::task::spawn_blocking(move || {
        super::remove(&directory.signed())?;
        for folder in ["ledger", "attachments", "journals", "specs", "pages", "tampering"] {
            super::remove(&directory.path().join(folder))?;
        }
        match std::fs::remove_file(directory.path().join("refused.json")) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        Ok(())
    })
    .await
    .map_err(io::Error::other)?
}

pub(super) async fn remove(node: &Arc<Node>, town: &TownName) -> io::Result<()> {
    let _syncing = node.syncing.lock().await;
    let directory = node.world.town(town);
    tokio::task::spawn_blocking(move || {
        // The signed entries go first: without them the folder is no
        // project any more, and the guard puts nothing back.
        super::remove(&directory.signed())?;
        super::remove(&directory.ledger())?;
        super::remove(&directory.attachments())?;
        for folder in ["journals", "specs", "pages", "tampering"] {
            super::remove(&directory.path().join(folder))?;
        }
        for file in ["project.json", "refused.json", "index.json"] {
            match std::fs::remove_file(directory.path().join(file)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        let _ = std::fs::remove_dir(directory.path());
        Ok(())
    })
    .await
    .map_err(io::Error::other)?
}
