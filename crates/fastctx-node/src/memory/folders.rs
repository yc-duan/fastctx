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
    if !crate::placement::owned(view.me(), &asker.account) {
        return Err(Failure::refused(fastctx_types::memory::words::OWN_COMPUTER_PATHS));
    }
    crate::town::access::check(node, &Acting::one(asker.hub.clone(), asker.account.clone()), town)?;
    if !node.world.town(town).signed().is_dir() {
        return Err(Failure::new(fastctx_types::memory::words::REPLICA_NOT_READY));
    }
    Ok(here(node, town))
}

pub(crate) async fn get(
    node: &Arc<Node>,
    session: Option<(&fastctx_types::ResidentId, &fastctx_types::residents::Origin)>,
    town: &TownName,
    acting: &Acting,
) -> Result<Folders, String> {
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
    let selected = if let Some((id, origin)) = session {
        crate::placement::location::prepare(node, id, origin, acting)
            .await
            .map_err(|error| error.message)?
            .map(|location| location.machine)
    } else {
        let choices = crate::placement::destinations(node, &Acting::one(place.hub.clone(), account.clone()))
            .map_err(|error| error.message)?;
        crate::placement::select(&choices, &Default::default(), Some(&node.machine()), None, None)
            .map_err(|error| error.message)?
            .map(|choice| choice.machine.name.clone())
    };
    let machine = selected.as_ref().ok_or(fastctx_types::memory::words::OFFLINE_COMPUTERS)?;
    if node.is_me(machine) {
        let asker = crate::admission::Asker {
            machine: node.machine(),
            hub: place.hub.clone(),
            account: account.clone(),
            hands: true,
        };
        return here_for(node, &asker, town).map_err(|error| error.message);
    }
    // Whether that computer can be asked is said by the cached read.
    let view = link.view();
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
    if !view.member(machine).is_some_and(|member| member.online && crate::placement::owned(Some(member), &account))
        || view.admits(&view.me, &account, machine, Purpose::Hands).is_err()
    {
        return Err(fastctx_types::memory::words::PATH_ACCESS_ENDED.into());
    }
    Ok(folders)
}
