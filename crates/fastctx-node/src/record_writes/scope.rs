//! Pin account selection at admission, and check its current incarnation on use.

use crate::{Node, town::caller::Caller};
use fastctx_net::link::{Link, Status};
use fastctx_types::{AccountName, HubName, access::HubAccount, ipc::Failure};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Target {
    pub scope: HubAccount,
    opened: jiff::Timestamp,
}

#[derive(Clone, Copy)]
pub(super) enum Authority {
    Acting,
    LoggedIn,
}

pub(super) fn capture(link: &Link, account: AccountName, authority: Authority) -> Result<Target, Failure> {
    let view = crate::authorization::current_view(link)?;
    match authority {
        Authority::Acting if !view.may_act_for(&account, &view.me) => {
            return Err(Failure::refused("this device no longer works for the account"));
        }
        Authority::LoggedIn if !view.logged_in(&view.me, &account) => {
            return Err(Failure::new(format!("this device is not logged in to the account {account}")));
        }
        _ => {}
    }
    let found = view.account(&account).ok_or_else(|| Failure::new(format!("there is no account {account}")))?;
    Ok(Target {
        scope: HubAccount { hub: link.membership().id.clone(), account: found.name.clone() },
        opened: found.opened,
    })
}

/// Waits while the hubs an account may be chosen on may soon give their
/// views. A hub that is connected and has not given its view leaves its
/// accounts unknown, not none, so nothing is chosen then; one that cannot
/// be reached is judged as the original selection judges it, at once.
pub(super) async fn selection(node: &Node, hub: Option<&HubName>) -> Result<(), Failure> {
    let links: Vec<_> = node.links().into_iter().filter(|link| hub.is_none_or(|hub| link.hub_name() == *hub)).collect();
    crate::reach::settled_each(&links).await;
    if links.iter().any(|link| matches!(link.status(), Status::Online { .. }) && !link.view_current()) {
        return Err(Failure::not_run("the hub's current account view did not arrive within a minute"));
    }
    Ok(())
}

/// Waits while the chosen account's hub may soon give its view. A hub this
/// computer is no longer on is left to `current`, which says so.
pub(super) async fn ready(node: &Node, target: &Target) -> Result<(), Failure> {
    let Some(link) = node.link_of(&target.scope.hub) else { return Ok(()) };
    crate::authorization::awaited_view(&link)
        .await
        .map(|_| ())
        .map_err(|_| Failure::not_run("the hub's current account view is unavailable"))
}

pub(super) fn resolve(
    node: &Node,
    caller: &Caller,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
) -> Result<Target, Failure> {
    let (link, account) = crate::profile::scope(node, caller, hub, account)?;
    capture(&link, account, Authority::Acting)
}

pub(super) fn current(
    node: &Node,
    caller: &Caller,
    target: &Target,
    authority: Authority,
) -> Result<Arc<Link>, Failure> {
    let link = node.link_of(&target.scope.hub).ok_or_else(|| Failure::refused("the selected hub is unavailable"))?;
    // A missing view is unknown authority, not an empty set of logged-in accounts.
    crate::authorization::current_view(&link)?;
    if matches!(authority, Authority::Acting) {
        let (selected, _) = crate::profile::scope(node, caller, Some(&link.hub_name()), Some(&target.scope.account))?;
        if selected.membership().id != target.scope.hub {
            return Err(Failure::refused("the selected hub was replaced"));
        }
    }
    let now = capture(&link, target.scope.account.clone(), authority)?;
    if now.opened != target.opened {
        return Err(Failure::refused("the selected account was replaced"));
    }
    Ok(link)
}
