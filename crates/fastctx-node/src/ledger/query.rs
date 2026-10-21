//! Projects this machine spans are kept here as they change; others are
//! taken on demand.

use std::collections::BTreeMap;
use std::sync::Arc;

use fastctx_types::TownName;
use fastctx_types::access::Acting;
use fastctx_types::ipc::Failure;
use fastctx_types::project::{Record, TownSummary};

use crate::Node;
use crate::town::read;

/// A project that spans this machine is kept here already. A wider query
/// asks each hub for the projects this computer's accounts may use, and
/// takes those it does not have. Without a connection it returns the copies
/// already here; `fastctx nodes` reports the disconnected state. Explicit
/// refusals and corrupt data still fail.
pub async fn query(node: &Arc<Node>, town: Option<&TownName>) -> Result<Vec<TownName>, String> {
    let local = available(node, town).await?;
    if let Some(town) = town
        && local.get(town).is_some_and(|record| record.machines.contains(&node.machine()))
    {
        return Ok(vec![town.clone()]);
    }
    let _syncing = node.syncing.lock().await;
    for link in node.links() {
        let Some(catalog) = link.try_ledger_catalog().await? else { continue };
        for (id, latest) in catalog {
            let Some(name) = link.view().project(&id).map(|project| project.name.clone()) else { continue };
            if town.is_some_and(|town| *town != name) {
                continue;
            }
            let have = crate::ledger::seq(&node.world, &name).map_err(|error| format!("reading {name}: {error}"))?;
            if have >= latest {
                continue;
            }
            let name = super::local_name(node, &link, &id)?;
            super::take(node, &link, &name, &id, latest).await?;
        }
    }
    Ok(available(node, town).await?.into_keys().collect())
}

/// The projects this machine has a record of, kept for a hub it is on, or
/// for itself.
pub(crate) async fn available(node: &Node, town: Option<&TownName>) -> Result<BTreeMap<TownName, Record>, String> {
    let world = node.world.clone();
    let mut hubs: Vec<_> = node.links().iter().map(|link| link.membership().id.clone()).collect();
    hubs.push(super::alone(&world).map_err(|error| format!("reading this computer's own id: {error}"))?);
    let town = town.cloned();
    tokio::task::spawn_blocking(move || {
        let names = match town {
            Some(town) => vec![town],
            None => world.towns()?,
        };
        let mut records = BTreeMap::new();
        for name in names {
            let Some(place) = super::names::place(&world, &name)? else { continue };
            if !hubs.contains(&place.hub) {
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
    let towns = query(node, None).await.map_err(Failure::new)?;
    summaries(node, towns).await
}

/// The projects whoever acts as `acting` may use: a hub's, where one of
/// the accounts it works for there may use it; this computer's own, kept on
/// no hub, only for whoever works for this computer's logins
/// (`world-network.md`, 一个居民替谁干活).
pub async fn list_for(node: &Arc<Node>, acting: &Acting) -> Result<Vec<TownSummary>, Failure> {
    let towns = list(node).await?;
    let world = node.world.clone();
    let names: Vec<TownName> = towns.iter().map(|summary| summary.town.clone()).collect();
    let places = tokio::task::spawn_blocking(move || {
        names.iter().map(|town| super::names::place(&world, town)).collect::<std::io::Result<Vec<_>>>()
    })
    .await
    .map_err(|error| Failure::new(format!("reading the projects: {error}")))?
    .map_err(|error| Failure::new(format!("reading the projects: {error}")))?;
    Ok(towns
        .into_iter()
        .zip(places)
        .filter(|(_, place)| place.as_ref().is_some_and(|place| sees(node, acting, place)))
        .map(|(summary, _)| summary)
        .collect())
}

/// Whether whoever acts as `acting` may use the project `town`, taken from
/// its hub first when this computer keeps nothing of it yet.
pub async fn usable(node: &Arc<Node>, acting: &Acting, town: &TownName) -> Result<bool, Failure> {
    query(node, Some(town)).await.map_err(Failure::new)?;
    let (world, name) = (node.world.clone(), town.clone());
    let place = tokio::task::spawn_blocking(move || super::names::place(&world, &name))
        .await
        .map_err(|error| Failure::new(format!("reading the project {town}: {error}")))?
        .map_err(|error| Failure::new(format!("reading the project {town}: {error}")))?;
    Ok(place.is_some_and(|place| sees(node, acting, &place)))
}

/// Whether one of the accounts `acting` works for may use the project at
/// `place`. A project kept on no hub is this computer's own, so its
/// owner's: whoever works for the account it is registered under.
fn sees(node: &Node, acting: &Acting, place: &super::names::Place) -> bool {
    if let Some(link) = node.link_of(&place.hub) {
        let view = link.view();
        return node.accounts_on(&link, acting).iter().any(|account| view.in_project(account, &place.town));
    }
    match acting {
        Acting::Here => true,
        Acting::Accounts { accounts } => {
            node.links().is_empty()
                || accounts.iter().any(|account| {
                    node.link_of(&account.hub)
                        .is_some_and(|link| link.view().me().and_then(|me| me.home.as_ref()) == Some(&account.account))
                })
        }
    }
}

/// The projects kept here of the accounts logged in here, asking no hub:
/// what an interface lists (`crate::shown`). One kept for an account this
/// computer only acts for, as one it assists, is not theirs to see
/// (`world-network.md`, 项目也照同一个规矩). Those this computer may use and
/// does not span are taken as they come and change (`follow`).
pub async fn held(node: &Arc<Node>) -> Result<Vec<TownSummary>, Failure> {
    let towns: Vec<TownName> = available(node, None).await.map_err(Failure::new)?.into_keys().collect();
    let world = node.world.clone();
    let names = towns.clone();
    let places = tokio::task::spawn_blocking(move || {
        names.iter().map(|town| super::names::place(&world, town)).collect::<std::io::Result<Vec<_>>>()
    })
    .await
    .map_err(|error| Failure::new(format!("reading the projects: {error}")))?
    .map_err(|error| Failure::new(format!("reading the projects: {error}")))?;
    let mut hubs = std::collections::HashMap::new();
    let mine = towns
        .into_iter()
        .zip(places)
        .filter(|(_, place)| place.as_ref().is_some_and(|place| sees(node, &Acting::Here, place)))
        .map(|(town, place)| {
            if let Some(place) = place {
                hubs.insert(town.clone(), place.hub);
            }
            town
        })
        .collect();
    let mut listed = summaries(node, mine).await?;
    // A project names its machines as they were called when it was written;
    // each is shown by its name now, as its hub's directory has it.
    for summary in &mut listed {
        let Some(view) = hubs.get(&summary.town).and_then(|hub| node.link_of(hub)).map(|link| link.view()) else {
            continue;
        };
        for machine in &mut summary.record.machines {
            *machine = view.canonical(machine).clone();
        }
        for place in &mut summary.record.places {
            place.location.machine = view.canonical(&place.location.machine).clone();
        }
        if let Some(canon) = &mut summary.record.canon {
            canon.machine = view.canonical(&canon.machine).clone();
        }
    }
    Ok(listed)
}

/// Takes, from `link`'s hub, `town`'s new entries when this computer keeps
/// it without spanning it; with no town, every project this computer may
/// use that it does not have whole, as when it connects again. A project
/// that spans it is taken as it changes already (`super::sync`).
pub async fn follow(
    node: &Arc<Node>,
    link: &Arc<fastctx_net::link::Link>,
    town: Option<&fastctx_types::access::TownId>,
) -> Result<(), String> {
    match town {
        None => query(node, None).await.map(|_| ()),
        Some(town) => {
            let Some(name) = link.view().project(town).map(|project| project.name.clone()) else { return Ok(()) };
            query(node, Some(&name)).await.map(|_| ())
        }
    }
}

/// Takes the projects `link`'s view names that this computer keeps nothing
/// of yet: one shared with an account here since.
pub async fn follow_view(node: &Arc<Node>, link: &Arc<fastctx_net::link::Link>) -> Result<(), String> {
    let names: Vec<TownName> = link.view().projects.iter().map(|project| project.name.clone()).collect();
    for name in names {
        let kept = super::names::place(&node.world, &name).map_err(|error| format!("reading {name}: {error}"))?;
        if kept.is_none() {
            query(node, Some(&name)).await?;
        }
    }
    Ok(())
}

async fn summaries(node: &Arc<Node>, towns: Vec<TownName>) -> Result<Vec<TownSummary>, Failure> {
    let alone = !node.connected_anywhere();
    let here = node.machine();
    let world = node.world.clone();
    tokio::task::spawn_blocking(move || {
        let mut listed = Vec::new();
        for town in towns {
            let record = read::latest(&world, &town)?
                .ok_or_else(|| std::io::Error::other(format!("{town} has no project record in its ledger")))?;
            if alone && !record.machines.contains(&here) {
                continue;
            }
            let journals = read::journal_names(&world, &town)?.len();
            listed.push(TownSummary { town, record, journals });
        }
        std::io::Result::Ok(listed)
    })
    .await
    .map_err(|error| Failure::new(format!("reading the projects: {error}")))?
    .map_err(|error| Failure::new(format!("reading the projects: {error}")))
}
