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
        .ok_or_else(|| Failure::new("the project has no ledger identity"))?;
    let Some(link) = node.link_of(&place.hub) else { return Ok(None) };
    let view = link.view();
    let project = view.project(&place.town).ok_or_else(|| Failure::new("the hub no longer lists the project"))?;
    let accounts: Vec<_> =
        node.accounts_on(&link, acting).into_iter().filter(|account| view.in_project(account, &place.town)).collect();
    let account = accounts
        .iter()
        .find(|account| **account == project.owner)
        .or(accounts.first())
        .ok_or_else(|| Failure::refused("no caller account may write this project"))?;
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
    node.shown_changed();
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
        return Ok(MemoryReply::Catalog { catalog: index.catalog });
    }
    node.authorize_here(&acting)?;
    let (world, town) = (node.world.clone(), request.town().clone());
    let store = tokio::task::spawn_blocking(move || super::store::read(&world, &town))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(error.to_string()))?;
    match request {
        MemoryCall::List { .. } => Ok(MemoryReply::Catalog { catalog: store.index.catalog() }),
        MemoryCall::Read { name, version, .. } => Ok(MemoryReply::Document {
            document: Box::new(store.index.document(&name, version, &store.files).map_err(Failure::new)?),
        }),
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
