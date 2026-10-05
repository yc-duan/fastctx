//! Replicated towns are read locally; other towns are queried on demand.

use std::collections::BTreeMap;
use std::sync::Arc;

use fastctx_types::TownName;
use fastctx_types::ipc::Failure;
use fastctx_types::project::{Record, TownSummary};

use crate::Node;
use crate::town::read;

/// A town that spans this machine already has background replication. A
/// World-wide query discovers other towns at the hub when connected. Without
/// that connection it returns the copies already here; Nodes reports the
/// disconnected state. Explicit refusals and corrupt data still fail.
pub async fn query(node: &Arc<Node>, town: Option<&TownName>) -> Result<Vec<TownName>, String> {
    let local = available(node, town).await?;
    if let Some(town) = town
        && local.get(town).is_some_and(|record| record.machines.contains(&node.machine()))
    {
        return Ok(vec![town.clone()]);
    }
    let Some(link) = node.link() else { return Ok(local.into_keys().collect()) };
    let _syncing = node.syncing.lock().await;
    let Some(catalog) = link.try_ledger_catalog().await? else { return Ok(local.into_keys().collect()) };
    let mut towns = Vec::new();
    for (name, latest) in catalog {
        if town.is_none_or(|requested| *requested == name) {
            let replicated = local.get(&name).is_some_and(|record| record.machines.contains(&node.machine()));
            if !replicated && !super::take(node, &link, &name, latest, true).await? {
                return Ok(available(node, town).await?.into_keys().collect());
            }
            towns.push(name);
        }
    }
    Ok(towns)
}

pub(crate) async fn available(node: &Node, town: Option<&TownName>) -> Result<BTreeMap<TownName, Record>, String> {
    let world = node.world.clone();
    let membership = node.link().map(|link| link.membership().world.clone());
    let town = town.cloned();
    tokio::task::spawn_blocking(move || {
        let names = match town {
            Some(town) => vec![town],
            None => world.towns()?,
        };
        let mut records = BTreeMap::new();
        for name in names {
            if let Some(expected) = &membership
                && super::origin(&world.town(&name))?.as_ref() != Some(expected)
            {
                continue;
            }
            if let Some(record) = read::latest(&world, &name)? {
                records.insert(name, record);
            }
        }
        std::io::Result::Ok(records)
    })
    .await
    .map_err(|error| format!("reading available projects: {error}"))?
    .map_err(|error| format!("reading available projects: {error}"))
}

pub async fn list(node: &Arc<Node>) -> Result<Vec<TownSummary>, Failure> {
    let standalone = node.link().is_none();
    let here = node.machine();
    let towns = query(node, None).await.map_err(Failure::new)?;
    let world = node.world.clone();
    tokio::task::spawn_blocking(move || {
        let mut listed = Vec::new();
        for town in towns {
            let record = read::latest(&world, &town)?
                .ok_or_else(|| std::io::Error::other(format!("{town} has no project record in its ledger")))?;
            if standalone && !record.machines.contains(&here) {
                continue;
            }
            let journals = read::journal_names(&world, &town)?.len();
            listed.push(TownSummary { town, record, journals });
        }
        std::io::Result::Ok(listed)
    })
    .await
    .map_err(|error| Failure::new(format!("reading the World directory: {error}")))?
    .map_err(|error| Failure::new(format!("reading the World directory: {error}")))
}
