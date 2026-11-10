//! Pin account selection at admission, and check its current incarnation on use.

use crate::{Node, town::caller::Caller};
use fastctx_net::link::Link;
use fastctx_types::{AccountName, HubName, access::HubAccount, ipc::Failure};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Target {
    pub scope: HubAccount,
    opened: jiff::Timestamp,
}

pub(super) fn capture(link: &Link, account: AccountName) -> Result<Target, Failure> {
    let view = crate::authorization::current_view(link)?;
    let found = view.account(&account).ok_or_else(|| Failure::refused("the account is unavailable"))?;
    if found.locked || !view.may_act_for(&account, &view.me) {
        return Err(Failure::refused("this device no longer works for the account"));
    }
    Ok(Target {
        scope: HubAccount { hub: link.membership().id.clone(), account: found.name.clone() },
        opened: found.opened,
    })
}

pub(super) fn resolve(
    node: &Node,
    caller: &Caller,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
) -> Result<Target, Failure> {
    let (link, account) = crate::profile::scope(node, caller, hub, account)?;
    capture(&link, account)
}

pub(super) fn current(node: &Node, caller: &Caller, target: &Target) -> Result<Arc<Link>, Failure> {
    let link = node.link_of(&target.scope.hub).ok_or_else(|| Failure::refused("the selected hub is unavailable"))?;
    let (selected, _) = crate::profile::scope(node, caller, Some(&link.hub_name()), Some(&target.scope.account))?;
    if selected.membership().id != target.scope.hub {
        return Err(Failure::refused("the selected hub was replaced"));
    }
    let now = capture(&link, target.scope.account.clone())?;
    if now.opened != target.opened {
        return Err(Failure::refused("the selected account was replaced"));
    }
    Ok(link)
}
