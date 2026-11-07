//! Resolve a history's resource ownership before selecting its storage.

use crate::{Node, town::caller::Caller};
use fastctx_net::link::Link;
use fastctx_types::{
    AccountName, Address,
    history::{Author, HistoryCall, HistoryReply, Scope},
    ipc::Failure,
};
use std::sync::Arc;

pub(crate) async fn author(node: &Arc<Node>, caller: &Caller) -> Result<Author, Failure> {
    crate::history::identity::author(node, caller).await
}

pub(crate) async fn network(
    node: &Arc<Node>,
    caller: &Caller,
    scope: &Scope,
) -> Result<Option<(Arc<Link>, AccountName)>, Failure> {
    let acting = crate::around::acting_of(node, caller);
    match scope {
        Scope::Project { project } => {
            crate::town::access::ready(node, &acting, project).await?;
            let place = crate::ledger::names::place(&node.world, project)
                .map_err(|error| Failure::new(error.to_string()))?
                .ok_or_else(|| Failure::new("the project has no ledger identity"))?;
            let Some(link) = node.link_of(&place.hub) else {
                return if place.hub
                    == crate::ledger::alone(&node.world).map_err(|error| Failure::new(error.to_string()))?
                {
                    Ok(None)
                } else {
                    Err(Failure::new("the project's hub is unavailable"))
                };
            };
            let account = node
                .accounts_on(&link, &acting)
                .into_iter()
                .find(|account| link.view().in_project(account, &place.town))
                .ok_or_else(|| Failure::refused("this account cannot use that project's history"))?;
            Ok(Some((link, account)))
        }
        Scope::Conversation { resident, machine } => {
            let address = Address { resident: resident.clone(), machine: machine.clone() };
            if node.is_me(machine) {
                if !crate::admission::seen::resident(node, crate::admission::seen::Looker::Here(&acting), resident) {
                    return Err(Failure::refused("this account cannot use that conversation's history"));
                }
                if let Some((about, _)) = node.residents.about(resident) {
                    crate::record_ownership::register(node, resident, &about.acting).await?;
                }
            }
            if !node.connected_anywhere() && node.is_me(machine) {
                return Ok(None);
            }
            for link in node.links() {
                for account in node.accounts_on(&link, &acting) {
                    if let Ok(fastctx_types::owned_records::Response::Conversation { .. }) = link
                        .owned_records(
                            &account,
                            fastctx_types::owned_records::Request::Resolve { conversation: address.clone() },
                        )
                        .await
                    {
                        return Ok(Some((link, account)));
                    }
                }
            }
            Err(Failure::not_run(
                "the conversation's history is unavailable; its hub must support shared records and its host must have registered it",
            ))
        }
    }
}

pub(crate) async fn call(node: &Arc<Node>, caller: &Caller, request: HistoryCall) -> Result<HistoryReply, Failure> {
    super::offered::check(node, caller)?;
    let changed = request.writes();
    let scope = request.scope().clone();
    let author = if request.writes() { Some(author(node, caller).await?) } else { None };
    let _history = super::import::lock(node, &scope).await;
    if super::import::pending(node, &scope) {
        let acting = crate::around::acting_of(node, caller);
        super::import::authorize(node, &acting, &scope)?;
        if !matches!(request, HistoryCall::List { .. }) {
            node.authorize_here(&acting)?;
        }
        super::import::schedule(node);
        if request.writes() {
            return Err(super::import::writing());
        }
        let mut reply = super::local::call(node, request, author).await?;
        redact(&mut reply, &|machine| node.authorize_machine(&acting, &machine.into()).is_ok());
        return Ok(reply);
    }
    let network = network(node, caller, request.scope()).await?;
    let acting = crate::around::acting_of(node, caller);
    if !matches!(request, HistoryCall::List { .. }) {
        node.authorize_here(&acting)?;
    }
    let mut reply = match network {
        Some((link, account)) => {
            let reply = link.history_call(&account, request, author).await.map_err(Failure::new)?;
            let usable = |machine: &fastctx_types::MachineName| link.view().rights(&account, machine).hands;
            let mut reply = reply;
            redact(&mut reply, &usable);
            reply
        }
        None => super::local::call(node, request, author).await?,
    };
    // Local callers may have a narrower work scope than their computer's logins.
    redact(&mut reply, &|machine| node.authorize_machine(&acting, &machine.into()).is_ok());
    if changed {
        crate::context::invalidations::history(node, &scope);
        node.memory_changed();
    }
    Ok(reply)
}

fn redact(reply: &mut HistoryReply, usable: &dyn Fn(&fastctx_types::MachineName) -> bool) {
    match reply {
        HistoryReply::Catalog { catalog } => catalog.for_reader(usable),
        HistoryReply::Document { document } => document.for_reader(usable),
        HistoryReply::Saved { revision } => revision.author.stamp.for_reader(usable),
    }
}
