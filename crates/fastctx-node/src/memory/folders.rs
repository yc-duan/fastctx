//! Resolve replica paths on the account's own computer, never on an assisting host.

use crate::Node;
use fastctx_types::{
    TownName,
    access::{Acting, Purpose},
    ipc::Failure,
    memory::Folders,
};
use std::sync::Arc;

pub(crate) fn here(node: &Node, town: &TownName) -> Folders {
    let directory = node.world.town(town);
    Folders {
        machine: node.machine(),
        journals: crate::world::native(&directory.journals()),
        specs: crate::world::native(&directory.specs()),
    }
}

pub(crate) fn here_for(node: &Node, asker: &crate::admission::Asker, town: &TownName) -> Result<Folders, Failure> {
    let link = node.link_of(&asker.hub).ok_or_else(|| Failure::refused(fastctx_types::memory::words::HUB_GONE))?;
    let view = link.view();
    if !view
        .me()
        .is_some_and(|member| member.home.as_ref() == Some(&asker.account) && member.logins.contains(&asker.account))
    {
        return Err(Failure::refused(fastctx_types::memory::words::OWN_COMPUTER_PATHS));
    }
    crate::town::access::check(node, &Acting::one(asker.hub.clone(), asker.account.clone()), town)?;
    if !node.world.town(town).signed().is_dir() {
        return Err(Failure::new(fastctx_types::memory::words::REPLICA_NOT_READY));
    }
    Ok(here(node, town))
}

pub(crate) async fn get(node: &Arc<Node>, town: &TownName, acting: &Acting) -> Result<Folders, String> {
    let place = crate::ledger::names::place(&node.world, town)
        .map_err(|error| error.to_string())?
        .ok_or(fastctx_types::memory::words::MISSING_IDENTITY)?;
    let Some(link) = node.link_of(&place.hub) else {
        node.works_here(acting)?;
        return Ok(here(node, town));
    };
    let account = super::api::account(node, acting, town)
        .map_err(|error| error.message)?
        .ok_or(fastctx_types::memory::words::MISSING_ACCOUNT)?;
    let view = link.view();
    if view.me().is_some_and(|member| member.logins.contains(&account)) && node.works_here(acting).is_ok() {
        return Ok(here(node, town));
    }
    let mut computers: Vec<_> = view
        .machines()
        .filter(|member| member.online && member.home.as_ref() == Some(&account) && member.logins.contains(&account))
        .filter(|member| view.admits(&view.me, &account, &member.name, Purpose::Hands).is_ok())
        .map(|member| member.name.clone())
        .collect();
    computers.sort();
    let machine = computers.first().ok_or(fastctx_types::memory::words::OFFLINE_COMPUTERS)?;
    let network_town = node
        .town_across(&link, Some(town))
        .map_err(|error| error.message)?
        .ok_or(fastctx_types::memory::words::MISSING_NETWORK_NAME)?;
    let ways = *node.ways.borrow();
    let memory = *node.memory_changes.borrow();
    let folders = node
        .folder_reads
        .fetch(super::folder_reads::Query {
            link: link.clone(),
            view,
            account: account.clone(),
            machine: machine.clone(),
            town: network_town,
            ways,
            memory,
        })
        .await?;
    let view = link.view();
    if !view.member(machine).is_some_and(|member| {
        member.online && member.home.as_ref() == Some(&account) && member.logins.contains(&account)
    }) || view.admits(&view.me, &account, machine, Purpose::Hands).is_err()
    {
        return Err(fastctx_types::memory::words::PATH_ACCESS_ENDED.into());
    }
    Ok(folders)
}
