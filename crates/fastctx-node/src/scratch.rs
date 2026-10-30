//! Scratch coordinates come from the machine that owns the directory. The
//! same layout lookup used by run preserves guest rooms across host renames.

pub(crate) mod jobs;
mod storage;
pub(crate) use storage::Receipt;

use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    access::{Acting, Purpose, Written},
    remote::{RemoteCall, RemoteReply},
    scratch::{Descriptions, words},
};
use std::{io, sync::Arc};

pub(crate) fn here(node: &Node, owner: &Address) -> String {
    let path = node.world.owner(&node.names(), owner).scratch();
    fastctx_hands::run::present::display_path(&path.to_string_lossy())
}

pub(crate) async fn descriptions(
    node: &Arc<Node>,
    id: &ResidentId,
    acting: &Acting,
    machines: Vec<String>,
) -> Descriptions {
    let mut pending = tokio::task::JoinSet::new();
    let mut tasks = std::collections::HashMap::new();
    for machine in machines {
        let (node, id, acting) = (node.clone(), id.clone(), acting.clone());
        let name = machine.clone();
        let task = pending.spawn(async move {
            async {
                let written = Written::parse(&machine).map_err(|error| error.to_string())?;
                let owner = Address { machine: node.machine(), resident: id };
                if node.is_here(&written) {
                    node.authorize_here(&acting).map_err(|error| error.message)?;
                    return Ok(here(&node, &owner));
                }
                let (link, account) =
                    node.speaking(&acting, &written, Purpose::Hands).await.map_err(|error| error.to_string())?;
                if !link.view().scratch_locations {
                    return Err(words::OLD_HUB.into());
                }
                if !link.view().member(&written.name).is_some_and(|member| member.online) {
                    return Err(words::OFFLINE.into());
                }
                match link
                    .call(&written.name, &account, Purpose::Hands, RemoteCall::Scratch { owner })
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.message)?
                {
                    RemoteReply::Scratch { path } => Ok(path),
                    _ => Err(words::WRONG_REPLY.into()),
                }
            }
            .await
        });
        tasks.insert(task.id(), name);
    }
    let mut descriptions = Descriptions::new();
    while let Some(result) = pending.join_next_with_id().await {
        let (task, path) = match result {
            Ok((task, path)) => (task, path),
            Err(error) => (error.id(), Err(error.to_string())),
        };
        descriptions.insert(tasks.remove(&task).expect("spawned scratch lookup"), path);
    }
    descriptions
}

#[derive(Default)]
pub(crate) struct Deliveries(std::sync::Mutex<std::collections::BTreeSet<ResidentId>>);

pub(crate) fn described(node: &Node, id: &ResidentId, paths: Descriptions) -> io::Result<()> {
    storage::observe(&node.world, id, paths, true)
}

/// Bind a direct executor's location to the name used for that call. Descendant
/// locations carry their hub identity so equal names remain distinguishable.
pub(crate) fn observed(
    node: &Node,
    id: &ResidentId,
    source: &Written,
    locations: &[fastctx_types::scratch::Location],
) -> io::Result<()> {
    let mut paths = Descriptions::new();
    if locations.is_empty() {
        paths.insert(source.to_string(), Err(words::UNREPORTED.into()));
    }
    for location in locations {
        let name = match &location.hub {
            None => source.to_string(),
            Some(hub) => {
                let link =
                    node.link_of(hub).ok_or_else(|| io::Error::other("scratch location names an unknown hub"))?;
                node.written(&link, &location.machine).to_string()
            }
        };
        paths.insert(name, location.path.clone());
    }
    storage::observe(&node.world, id, paths, false)
}

pub(crate) fn collect(node: &Node, id: &ResidentId, start: usize) -> io::Result<(Vec<String>, Receipt)> {
    storage::collect(&node.world, id, start)
}

pub(crate) fn reserve(
    node: &Node,
    id: &ResidentId,
    heard: &mut crate::residents::Heard,
) -> Option<std::ops::Range<usize>> {
    let receipt = heard.scratch.as_mut()?;
    if receipt.shown.is_empty() {
        return None;
    }
    if !node.scratch_deliveries.0.lock().expect("never poisoned").insert(id.clone()) {
        let shown = receipt.shown.clone();
        heard.scratch = None;
        return Some(shown);
    }
    receipt.reserved = true;
    None
}

pub(crate) fn release(node: &Node, id: &ResidentId, heard: &crate::residents::Heard) {
    if heard.scratch.as_ref().is_some_and(|receipt| receipt.reserved) {
        node.scratch_deliveries.0.lock().expect("never poisoned").remove(id);
    }
}

pub(crate) fn acknowledge(node: &Node, id: &ResidentId, heard: &mut crate::residents::Heard) -> io::Result<()> {
    if let Some(receipt) = &heard.scratch {
        storage::acknowledge(&node.world, id, receipt)?;
    }
    release(node, id, heard);
    if let Some(receipt) = &mut heard.scratch {
        receipt.reserved = false;
    }
    Ok(())
}
