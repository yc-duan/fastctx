//! Resolve replica paths on the account's own computer, never on an assisting host.

use crate::Node;
use fastctx_types::{
    TownName,
    access::{Acting, Purpose},
    ipc::Failure,
    memory::Folders,
    remote::{RemoteCall, RemoteReply},
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
    let facts = link
        .call(machine, &account, Purpose::Look, RemoteCall::Facts)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.message)?;
    let RemoteReply::Facts(facts) = facts else { return Err(fastctx_types::memory::words::COMPUTER_UNANSWERED.into()) };
    if !facts.memory_folders {
        return Err(fastctx_types::memory::words::paths_old(machine));
    }
    let town = node
        .town_across(&link, Some(town))
        .map_err(|error| error.message)?
        .ok_or(fastctx_types::memory::words::MISSING_NETWORK_NAME)?;
    let reply = link
        .call(machine, &account, Purpose::Hands, RemoteCall::MemoryFolders { town })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.message)?;
    let RemoteReply::MemoryFolders(folders) = reply else {
        return Err(fastctx_types::memory::words::PATHS_UNANSWERED.into());
    };
    let view = link.view();
    if !view.member(machine).is_some_and(|member| member.online && crate::placement::owned(Some(member), &account))
        || view.admits(&view.me, &account, machine, Purpose::Hands).is_err()
    {
        return Err(fastctx_types::memory::words::PATH_ACCESS_ENDED.into());
    }
    Ok(folders)
}
