//! Lets go of the projects of a hub this computer may keep no longer: one
//! deleted, one no account it acts for may read since a sharing was taken
//! back or the project was handed on. A copy stays while an account this
//! computer acts for may read the project, or while the project spans it;
//! the conversations here still in a project let go of move out of it
//! first (`world-network.md`, 项目也照同一个规矩).

use std::io;
use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::TownName;
use fastctx_types::management::Management;

use crate::Node;
use crate::town::read;

/// Lets go of every project of `link`'s hub this computer may no longer
/// keep, as its view now says.
pub async fn let_go(node: &Arc<Node>, link: &Arc<Link>) {
    if let Err(error) = super::deletion::finish(node, link).await {
        node.log.line(format_args!("finishing project deletions: {error}"));
        return;
    }
    let hub = link.membership().id.clone();
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
            view.project(&place.town).is_some()
                && (view.member_in_project(&me, &place.town) || spans(node, &town, |machine| view.same(machine, &me)))
        };
        if kept {
            continue;
        }
        let living: Vec<_> = node
            .residents
            .everyone()
            .filter(|resident| resident.removed.is_none() && resident.town.as_ref() == Some(&town))
            .map(|resident| resident.id)
            .collect();
        let mut stuck = false;
        for id in living {
            if let Err(failure) = crate::management::here(node, &id, Management::Move { town: None }).await {
                node.log.line(format_args!("moving {id} out of {town}, which is let go of: {}", failure.message));
                stuck = true;
            }
        }
        // A conversation still in it keeps it until the view changes again.
        if stuck {
            continue;
        }
        match remove(node, &town).await {
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

/// Whether the latest record of `town` says it spans this computer.
fn spans(node: &Node, town: &TownName, is_me: impl Fn(&fastctx_types::MachineName) -> bool) -> bool {
    read::latest(&node.world, town).ok().flatten().is_some_and(|record| record.machines.iter().any(is_me))
}

/// Removes what this computer kept of `town`: its ledger, its signed
/// entries, attachments, pages and records; the folder itself only when
/// nothing else is left in it, as a conversation may stand there.
pub(super) async fn remove(node: &Arc<Node>, town: &TownName) -> io::Result<()> {
    let _syncing = node.syncing.lock().await;
    let directory = node.world.town(town);
    tokio::task::spawn_blocking(move || {
        // The signed entries go first: without them the folder is no
        // project any more, and the guard puts nothing back.
        super::remove(&directory.signed())?;
        super::remove(&directory.ledger())?;
        super::remove(&directory.attachments())?;
        for folder in ["pages", "tampering"] {
            super::remove(&directory.path().join(folder))?;
        }
        for file in ["project.json", "refused.json"] {
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
