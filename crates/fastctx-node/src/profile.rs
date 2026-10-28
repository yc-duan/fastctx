//! Resolve configuration through the caller's account, never the agent login.

use crate::{Node, town::caller::Caller};
use fastctx_net::link::Link;

use fastctx_types::ipc::Failure;
use fastctx_types::{AccountName, HubName};
pub(crate) mod session;
pub(crate) mod subagents;

use std::sync::Arc;

pub(crate) fn scope(
    node: &Node,
    caller: &Caller,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
) -> Result<(Arc<Link>, AccountName), Failure> {
    let acting = crate::around::acting_of(node, caller);
    if matches!(caller, Caller::Page(_) | Caller::Session(_)) {
        return Err(Failure::refused("this caller has no account configuration"));
    }
    let mut choices = Vec::new();
    for link in node.links().into_iter().filter(|link| hub.is_none_or(|hub| link.hub_name() == *hub)) {
        for name in node.accounts_on(&link, &acting) {
            if account.is_none_or(|account| *account == name) {
                choices.push((link.clone(), name));
            }
        }
    }
    match choices.len() {
        1 => Ok(choices.pop().expect("one account")),
        0 => Err(Failure::refused("this caller works for no matching account")),
        _ => Err(Failure::not_run("choose the account with --as and its hub with --hub")),
    }
}
