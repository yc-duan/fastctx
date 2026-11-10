//! Page authorization that needs no refresh or complete ledger scan.
use crate::{Node, admission::Asker, town::caller::Caller};
use fastctx_types::{
    Call, MachineName, TownName,
    access::{Acting, HubAccount, Purpose},
    ipc::Failure,
    pages::{PageAct, PageCall, PageIdentity, PageInvocation},
    remote::RemoteCall,
};
use std::sync::Arc;

pub(super) fn changes(call: &PageCall) -> bool {
    matches!(
        call,
        PageCall::Publish { .. }
            | PageCall::Withdraw { .. }
            | PageCall::Restore { .. }
            | PageCall::Consent { .. }
            | PageCall::Act { .. }
    )
}

pub(super) fn explicit(call: &PageCall) -> bool {
    changes(call)
        && call.town().is_some()
        && (!matches!(call, PageCall::Consent { .. } | PageCall::Act { .. }) || call.account().is_some())
}

pub(super) fn check(
    node: &Arc<Node>,
    caller: &Caller,
    opened_on: &MachineName,
    call: &PageCall,
) -> Result<String, Failure> {
    match call {
        PageCall::Publish { base, draft, .. } => {
            crate::pages::publisher(caller)?;
            crate::pages::next(*base)?;
            for act in &draft.acts {
                declaration(act)?;
            }
        }
        PageCall::Withdraw { base, .. } | PageCall::Restore { base, .. } => {
            crate::pages::publisher(caller)?;
            crate::pages::next(Some(*base))?;
        }
        PageCall::Consent { base, .. } => {
            crate::pages::user(caller)?;
            crate::pages::next(*base)?;
        }
        PageCall::Act { .. } => crate::pages::user(caller)?,
        _ => return Err(Failure::not_run("this page call is not a change")),
    }
    let town = super::town(node, caller, call.town())?;
    if let Some(account) = call.account()
        && !node.links().iter().any(|link| link.view().logged_in(opened_on, account))
    {
        return Err(Failure::refused("the page account is not logged in on its opening device"));
    }
    if let PageCall::Act { action, .. } = call {
        invocation(node, &town, action, false)?;
    }
    let page = match call {
        PageCall::Publish { page, .. }
        | PageCall::Withdraw { page, .. }
        | PageCall::Restore { page, .. }
        | PageCall::Consent { page, .. }
        | PageCall::Act { page, .. } => page,
        _ => unreachable!("page changes only"),
    };
    page_unit(&town, page)
}
fn page_unit(town: &TownName, page: &fastctx_types::PageName) -> Result<String, Failure> {
    // Town handles are unique on this node before and after ledger discovery.
    super::unit(&("page", town, page))
}
fn declaration(act: &PageAct) -> Result<(), Failure> {
    match act {
        PageAct::Run { script, .. } if script.trim().is_empty() => {
            Err(Failure::new("a page run needs its exact nonempty command"))
        }
        PageAct::Journal { .. } => Err(Failure::not_run(fastctx_types::memory::LEGACY)),
        _ => Ok(()),
    }
}
fn invocation(node: &Arc<Node>, town: &TownName, action: &PageInvocation, here: bool) -> Result<(), Failure> {
    declaration(&action.act())?;
    let target = match action {
        PageInvocation::Run { machine, .. } => machine,
        PageInvocation::Say { to, .. } => &to.machine,
        PageInvocation::Journal { .. } => return Err(Failure::not_run(fastctx_types::memory::LEGACY)),
    };
    // Publication and recipient lifecycle checks belong to execution. Reasking
    // an already completed delivery must still retrieve its original result.
    if crate::ledger::names::place(&node.world, town).map_err(|error| Failure::new(error.to_string()))?.is_some() {
        let destination = crate::town::machines::destination(node, town, target)?;
        if here && !node.is_here(&destination) {
            return Err(Failure::not_run("the page action is for another machine"));
        }
    }
    Ok(())
}

pub(super) fn remote_check(node: &Arc<Node>, asker: &Asker, call: &RemoteCall) -> Result<String, Failure> {
    match call {
        RemoteCall::PageAction { source, action } => {
            if source.identity != Some(PageIdentity::Account { hub: asker.hub.clone(), account: asker.account.clone() })
            {
                return Err(Failure::refused("the page action's account differs from its sender"));
            }

            invocation(node, &source.town, action, true)?;
            page_unit(&source.town, &source.page)
        }
        RemoteCall::Relay { made } => {
            let Call::Page(call) =
                serde_json::from_value::<Call>(made.clone()).map_err(|error| Failure::not_run(error.to_string()))?
            else {
                return Err(Failure::not_run("this relay is not a page change"));
            };
            if !matches!(
                call,
                PageCall::Consent { .. } | PageCall::Act { .. } | PageCall::Withdraw { .. } | PageCall::Restore { .. }
            ) {
                return Err(Failure::not_run("a device cannot relay this page call"));
            }
            if call.account().is_some_and(|account| account != &asker.account) {
                return Err(Failure::refused("the page account differs from the authenticated device account"));
            }
            let town = call.town().ok_or_else(|| Failure::not_run("a device names the project it means"))?;
            let scope = HubAccount { hub: asker.hub.clone(), account: asker.account.clone() };
            let link =
                node.link_of(&scope.hub).ok_or_else(|| Failure::refused("the requesting network is unavailable"))?;
            let view = crate::authorization::current_view(&link)?;
            if !crate::ledger::names::town_id(node, town).is_some_and(|id| view.in_project(&asker.account, &id)) {
                return Err(Failure::refused("the device account may not use the project"));
            }
            crate::town::access::check(node, &Acting::one(scope.hub, scope.account), town)?;
            if let PageCall::Act { action, .. } = &call {
                let (target, purpose) = match action {
                    PageInvocation::Run { machine, .. } => (machine, Purpose::Hands),
                    PageInvocation::Say { to, .. } => (&to.machine, Purpose::Words),
                    PageInvocation::Journal { .. } => return Err(Failure::not_run(fastctx_types::memory::LEGACY)),
                };
                view.admits(&asker.machine, &asker.account, target, purpose).map_err(Failure::refused)?;
            }
            let mut call = call;
            call.for_account(asker.account.clone());
            check(node, &Caller::User, &asker.machine, &call)
        }
        _ => Err(Failure::not_run("this is not a page change")),
    }
}
