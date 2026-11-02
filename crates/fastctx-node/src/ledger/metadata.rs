//! An assisting host keeps project metadata in memory, never as a disk replica.

use std::io;
use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::TownName;
use fastctx_types::access::TownId;
use fastctx_types::layout::WorldDir;
use fastctx_types::memory::ProjectIndex;

use crate::Node;

type Indices = std::collections::HashMap<std::path::PathBuf, ProjectIndex>;
static INDICES: std::sync::LazyLock<std::sync::Mutex<Indices>> = std::sync::LazyLock::new(Default::default);

pub(crate) fn read(world: &WorldDir, town: &TownName) -> io::Result<Option<ProjectIndex>> {
    Ok(INDICES.lock().expect("never poisoned").get(world.town(town).path()).cloned())
}

pub(crate) fn forget(world: &WorldDir, town: &TownName) {
    INDICES.lock().expect("never poisoned").remove(world.town(town).path());
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
        match fastctx_platform::disk::remove_file(node.world.town(&town).path().join("index.json")) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        if read(&node.world, &town)
            .map_err(|error| error.to_string())?
            .as_ref()
            .is_some_and(|kept| kept.seq >= index.seq)
        {
            continue;
        }
        let (node, world) = (node.clone(), node.world.clone());
        fastctx_platform::task::spawn_blocking(move || {
            INDICES.lock().expect("never poisoned").insert(world.town(&town).path().to_path_buf(), index);
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
