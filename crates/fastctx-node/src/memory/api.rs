//! The node's account-aware project-memory interface.

use crate::{Node, town::caller::Caller};
use fastctx_net::ledger::File;
use fastctx_types::{
    AccountName, TownName,
    access::Acting,
    ipc::Failure,
    memory::{MemoryCall, MemoryReply},
};
use std::sync::Arc;

pub(crate) fn account(node: &Node, acting: &Acting, town: &TownName) -> Result<Option<AccountName>, Failure> {
    let place = crate::ledger::names::place(&node.world, town)
        .map_err(|error| Failure::new(error.to_string()))?
        .ok_or_else(|| Failure::new(fastctx_types::memory::words::MISSING_IDENTITY))?;
    let Some(link) = node.link_of(&place.hub) else { return Ok(None) };
    let view = link.view();
    let project =
        view.project(&place.town).ok_or_else(|| Failure::new(fastctx_types::memory::words::PROJECT_UNLISTED))?;
    let accounts: Vec<_> =
        node.accounts_on(&link, acting).into_iter().filter(|account| view.in_project(account, &place.town)).collect();
    let account = accounts
        .iter()
        .find(|account| **account == project.owner)
        .or(accounts.first())
        .ok_or_else(|| Failure::refused(fastctx_types::memory::words::NO_PROJECT_WRITER))?;
    Ok(Some(account.clone()))
}

pub(crate) async fn publish(
    node: &Arc<Node>,
    acting: &Acting,
    town: &TownName,
    files: Vec<File>,
) -> Result<(), Failure> {
    crate::town::access::check(node, acting, town)?;
    let account = account(node, acting, town)?;
    crate::ledger::write_memory(node, town, files, account).await?;
    node.memory_changed();
    Ok(())
}

/// Reads an already synchronized catalog without creating another hub refresh.
pub(crate) async fn catalog(
    node: &Arc<Node>,
    caller: &Caller,
    town: &TownName,
) -> Result<fastctx_types::memory::Catalog, Failure> {
    catalog_for(node, &crate::around::acting_of(node, caller), town).await
}

pub(crate) async fn catalog_for(
    node: &Arc<Node>,
    acting: &Acting,
    town: &TownName,
) -> Result<fastctx_types::memory::Catalog, Failure> {
    crate::town::access::check(node, acting, town)?;
    let (world, name) = (node.world.clone(), town.clone());
    let mut catalog = tokio::task::spawn_blocking(move || super::store::catalog(&world, &name))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error: std::io::Error| Failure::new(error.to_string()))?;
    crate::town::access::check(node, acting, town)?;
    told(node, acting, town, |usable| catalog.for_reader(usable))?;
    Ok(catalog)
}

/// Tells what `tell` trims as whoever works for `acting` may know it: the
/// authors of the project `town`'s memory named only where they may work
/// (`ledger::query::usable`).
fn told(
    node: &Node,
    acting: &Acting,
    town: &TownName,
    tell: impl FnOnce(&dyn Fn(&fastctx_types::MachineName) -> bool),
) -> Result<(), Failure> {
    let usable = crate::ledger::query::usable(node, acting, town).map_err(|error| Failure::new(error.to_string()))?;
    if let Some(usable) = usable {
        tell(&*usable);
    }
    Ok(())
}

pub(crate) async fn call(node: &Arc<Node>, caller: &Caller, request: MemoryCall) -> Result<MemoryReply, Failure> {
    let acting = crate::around::acting_of(node, caller);
    crate::town::access::ready(node, &acting, request.town()).await?;
    if let MemoryCall::List { town } = &request
        && let Some(index) =
            crate::ledger::metadata::read(&node.world, town).map_err(|error| Failure::new(error.to_string()))?
        && !node.world.town(town).signed().is_dir()
    {
        let mut catalog = index.catalog;
        told(node, &acting, town, |usable| catalog.for_reader(usable))?;
        return Ok(MemoryReply::Catalog { catalog });
    }
    node.authorize_here(&acting)?;
    let (world, town) = (node.world.clone(), request.town().clone());
    let store = tokio::task::spawn_blocking(move || super::store::read(&world, &town))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(error.to_string()))?;
    crate::town::access::check(node, &acting, request.town())?;
    node.authorize_here(&acting)?;
    match request {
        MemoryCall::List { town } => {
            let mut catalog = store.index.catalog();
            told(node, &acting, &town, |usable| catalog.for_reader(usable))?;
            Ok(MemoryReply::Catalog { catalog })
        }
        MemoryCall::Read { town, name, version } => {
            let mut document = store.index.document(&name, version, &store.files).map_err(Failure::new)?;
            told(node, &acting, &town, |usable| document.for_reader(usable))?;
            Ok(MemoryReply::Document { document: Box::new(document) })
        }
        MemoryCall::Save { town, kind, name, text, expected } => {
            let author = super::identity::author(node, caller).await?;
            let (revision, files) =
                fastctx_net::memory::revision_files(kind, &name, &text, expected, author).map_err(Failure::new)?;
            store.index.check(&files).map_err(Failure::new)?;
            publish(node, &acting, &town, files).await?;
            Ok(MemoryReply::Saved { revision: Box::new(revision) })
        }
    }
}
