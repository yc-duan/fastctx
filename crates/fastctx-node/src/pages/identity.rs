//! Resolve a page's account once and retain it through actions and scripts.

use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::ipc::Failure;
use fastctx_types::pages::{PageIdentity, PageSource};
use fastctx_types::{AccountName, MachineName, TownName};

use crate::{Node, town::caller::Caller};

pub(super) fn resolve(
    node: &Node,
    caller: &Caller,
    opened_on: &MachineName,
    town: &TownName,
    selected: Option<&AccountName>,
) -> Result<PageIdentity, Failure> {
    let place = crate::ledger::names::place(&node.world, town)
        .map_err(|error| Failure::new(error.to_string()))?
        .ok_or_else(|| Failure::new(format!("there is no project {town}")))?;
    if place.hub == crate::ledger::alone(&node.world).map_err(|error| Failure::new(error.to_string()))? {
        if selected.is_some() || !node.is_me(opened_on) || !matches!(caller, Caller::User) {
            return Err(Failure::refused("a local page needs its local user's consent"));
        }
        return Ok(PageIdentity::Local { machine: node.machine() });
    }
    let link = node
        .link_of(&place.hub)
        .ok_or_else(|| Failure::refused("the page's account cannot be verified without its hub"))?;
    let view = link.view();
    let accounts = match caller {
        Caller::User => {
            view.member(opened_on).map(|member| member.logins.iter().cloned().collect::<Vec<_>>()).unwrap_or_default()
        }
        _ => node.accounts_on(&link, &crate::around::acting_of(node, caller)),
    };
    let mut eligible = accounts.into_iter().filter(|account| {
        selected.is_none_or(|selected| selected == account)
            && view.account(account).is_some_and(|account| !account.locked)
            && view.in_project(account, &place.town)
    });
    let account = eligible.next().ok_or_else(|| Failure::refused("no authenticated account may use this page"))?;
    if eligible.next().is_some() {
        return Err(Failure::refused("choose the account whose page consent to use"));
    }
    Ok(PageIdentity::Account { hub: place.hub, account })
}

/// An environment claim must match a live job the node admitted.
pub(crate) fn job(node: &Node, source: &PageSource, job: Option<&fastctx_types::JobId>) -> Result<(), Failure> {
    let job = job.ok_or_else(|| {
        Failure::refused("the page command has no job identity; upgrade Fastctx and run the action again")
    })?;
    let owner = fastctx_types::run::JobOwner::Page { page: source.clone() };
    let path = node.world.job_record(&node.names(), Some(&source.town), &owner, job);
    let record = fastctx_hands::run::record::read(&path)
        .map_err(|error| Failure::refused(format!("the page job cannot be identified: {error}")))?;
    if record.owner != owner || record.end.is_some() || record.account != source.identity.as_ref().and_then(account) {
        return Err(Failure::refused("the page command does not belong to this running job"));
    }
    Ok(())
}

pub(crate) fn acting(node: &Node, source: &PageSource) -> Result<Acting, Failure> {
    match source.identity.as_ref() {
        Some(PageIdentity::Account { hub, account }) => {
            let place = crate::ledger::names::place(&node.world, &source.town)
                .map_err(|error| Failure::new(error.to_string()))?
                .ok_or_else(|| Failure::refused("the page's project is no longer known"))?;
            let link = node.link_of(hub).ok_or_else(|| Failure::refused("the page's account is not connected"))?;
            let view = link.view();
            if place.hub != *hub
                || view.account(account).is_none_or(|account| account.locked)
                || !view.in_project(account, &place.town)
                || !view.logged_in(&source.opened_on, account)
            {
                return Err(Failure::refused("the page's account no longer has access"));
            }
            Ok(Acting::one(hub.clone(), account.clone()))
        }
        Some(PageIdentity::Local { machine }) if node.is_me(machine) && node.is_me(&source.opened_on) => {
            let place = crate::ledger::names::place(&node.world, &source.town)
                .map_err(|error| Failure::new(error.to_string()))?;
            if place.is_some_and(|place| crate::ledger::alone(&node.world).ok().as_ref() == Some(&place.hub)) {
                Ok(Acting::Here)
            } else {
                Err(Failure::refused("this page no longer belongs to a local project"))
            }
        }
        _ => Err(Failure::refused("the page action has no account identity; open the page and consent again")),
    }
}

pub(super) fn account(identity: &PageIdentity) -> Option<HubAccount> {
    match identity {
        PageIdentity::Account { hub, account } => Some(HubAccount { hub: hub.clone(), account: account.clone() }),
        PageIdentity::Local { .. } => None,
    }
}
