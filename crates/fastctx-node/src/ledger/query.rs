//! On-demand ledger copies for the same World query at any machine.

use std::sync::Arc;

use fastctx_types::TownName;
use fastctx_types::ipc::Failure;
use fastctx_types::project::TownSummary;

use crate::Node;
use crate::town::read;

/// Takes the requested town, or the complete directory, through the hub's
/// advertised cursors. Background replication still follows town membership.
/// An unreachable hub is an error, never an apparently complete stale list.
pub async fn query(node: &Arc<Node>, town: Option<&TownName>) -> Result<Vec<TownName>, String> {
    let Some(link) = node.link() else {
        return node.world.towns().map_err(|error| error.to_string());
    };
    let _syncing = node.syncing.lock().await;
    let catalog = link.ledger_catalog().await?;
    let mut towns = Vec::new();
    for (name, latest) in catalog {
        if town.is_none_or(|requested| *requested == name) {
            super::take(node, &link, &name, latest, true).await?;
            towns.push(name);
        }
    }
    Ok(towns)
}

pub async fn list(node: &Arc<Node>) -> Result<Vec<TownSummary>, Failure> {
    let standalone = node.link().is_none();
    let here = node.machine();
    let towns = query(node, None).await.map_err(Failure::new)?;
    let world = node.world.clone();
    tokio::task::spawn_blocking(move || {
        let mut listed = Vec::new();
        for town in towns {
            let Some(record) = read::latest(&world, &town)? else {
                if standalone {
                    continue;
                }
                return Err(std::io::Error::other(format!("{town} has no project record in its ledger")));
            };
            if standalone && !record.machines.contains(&here) {
                continue;
            }
            let journals = read::journal_names(&world, &town)?.len();
            listed.push(TownSummary { town, record, journals });
        }
        Ok(listed)
    })
    .await
    .map_err(|error| Failure::new(format!("reading the World directory: {error}")))?
    .map_err(|error| Failure::new(format!("reading the World directory: {error}")))
}
