//! An assisting host keeps only the project's attributes and memory catalog.

use std::io;
use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::TownName;
use fastctx_types::access::TownId;
use fastctx_types::layout::WorldDir;
use fastctx_types::memory::ProjectIndex;

use crate::Node;

pub(crate) fn read(world: &WorldDir, town: &TownName) -> io::Result<Option<ProjectIndex>> {
    let path = world.town(town).path().join("index.json");
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Fetches without local gates, then applies under the synchronization gate.
pub(crate) async fn refresh(node: &Arc<Node>, link: &Arc<Link>, wanted: Option<&TownId>) -> Result<(), String> {
    let view = link.view();
    let accounts = view.acts_for(&view.me);
    let projects: Vec<_> = view
        .projects
        .iter()
        .filter(|project| wanted.is_none_or(|wanted| *wanted == project.town))
        .filter(|project| !view.member_keeps_project(&view.me, &project.town))
        .filter_map(|project| {
            accounts
                .iter()
                .find(|account| view.in_project(account, &project.town))
                .map(|account| (project.town.clone(), account.clone()))
        })
        .collect();
    drop(view);
    for (id, account) in projects {
        let index = link.project_index(&account, &id).await?;
        let _syncing = node.syncing.lock().await;
        let view = link.view();
        if !view.may_act_for(&account, &view.me) || !view.in_project(&account, &id) {
            return Err(fastctx_types::memory::words::INDEX_ACCESS_ENDED.into());
        }
        if view.member_keeps_project(&view.me, &id) {
            continue;
        }
        let town = super::local_name(node, link, &id)?;
        if read(&node.world, &town)
            .map_err(|error| error.to_string())?
            .as_ref()
            .is_some_and(|kept| kept.seq >= index.seq)
        {
            continue;
        }
        let (node, world) = (node.clone(), node.world.clone());
        tokio::task::spawn_blocking(move || {
            fastctx_net::link::state::write(&world.town(&town).path().join("index.json"), &index)?;
            crate::town::instructions::refresh(&node, &town);
            node.memory_changed();
            io::Result::Ok(())
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}
