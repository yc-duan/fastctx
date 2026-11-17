//! Resolve a plan's account before queueing, preserving observed and remote-route rules.
use crate::{Node, town::caller::Caller};
use fastctx_types::{
    AccountName, Address, HubName, ResidentId,
    access::{HubAccount, Purpose},
    ipc::Failure,
    plan::{Operation, words},
    residents::About,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Scope {
    pub account: HubAccount,
    observed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum Target {
    Here { resident: ResidentId, scope: Scope, preparation: Option<crate::placement::location::selection::Selected> },
    Remote { address: Address, scope: HubAccount },
}
impl Scope {
    pub(crate) fn check(&self, node: &Node, id: &ResidentId) -> Result<(), Failure> {
        let (about, _) = node.residents.about(id).ok_or_else(|| Failure::refused(words::RESIDENT))?;
        self.check_about(node, &about)
    }
    fn check_about(&self, node: &Node, about: &About) -> Result<(), Failure> {
        let actor = &about.acting;
        let allowed = node.serves(actor, &self.account.hub, &self.account.account)
            || (self.observed
                && crate::record_ownership::allows(node, about, &self.account.hub, &self.account.account));
        let link = node.link_of(&self.account.hub).ok_or_else(|| Failure::refused(words::ROUTE))?;
        let view = crate::authorization::current_view(&link)?;
        if !allowed || view.account(&self.account.account).is_none_or(|account| account.locked) {
            return Err(Failure::refused(words::ROUTE));
        }
        Ok(())
    }
}
pub(super) fn here(
    node: &Node,
    id: &ResidentId,
    about: &About,
    previous: Option<&HubAccount>,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
    requesting: Option<&HubAccount>,
) -> Result<Scope, Failure> {
    let selected = requesting.or_else(|| if hub.is_none() && account.is_none() { previous } else { None });
    let chosen = match selected {
        Some(scope) => Scope { account: scope.clone(), observed: true },
        None => {
            let caller = Caller::Resident(Address { machine: node.machine(), resident: id.clone() });
            let (link, account) = crate::profile::scope(node, &caller, hub, account)?;
            Scope { account: HubAccount { hub: link.membership().id, account }, observed: false }
        }
    };
    chosen.check_about(node, about)?;
    Ok(chosen)
}

pub(crate) async fn remote(
    node: &Arc<Node>,
    address: &Address,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
) -> Result<HubAccount, Failure> {
    let (reading, owner) = (node.clone(), address.clone());
    let running = fastctx_platform::task::spawn_blocking(move || {
        crate::jobs::running_for(&reading.world, &reading.names(), &owner)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?;
    let mut choices = Vec::new();
    for scope in running {
        let Some(link) = node.link_of(&scope.hub) else { continue };
        if hub.is_some_and(|hub| link.hub_name() != *hub) || account.is_some_and(|account| *account != scope.account) {
            continue;
        }
        link.refresh().await.map_err(Failure::new)?;
        let view = link.view();
        if view.admits(&view.me, &scope.account, &address.machine, Purpose::Echo).is_ok() {
            choices.push(scope);
        }
    }
    if choices.len() != 1 {
        return Err(Failure::refused(words::ROUTE));
    }
    let scope = choices.pop().expect("one route");
    if !node.link_of(&scope.hub).is_some_and(|link| link.view().conversation_plans) {
        return Err(Failure::not_run(words::OLDER));
    }
    Ok(scope)
}

pub(crate) async fn resolve(
    node: &Arc<Node>,
    caller: &Caller,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
    operation: &Operation,
) -> Result<Target, Failure> {
    let Caller::Resident(address) = caller else {
        return Err(Failure::not_run(words::RESIDENT));
    };
    if !node.is_me(&address.machine) {
        return Ok(Target::Remote { address: address.clone(), scope: remote(node, address, hub, account).await? });
    }
    resolve_here(node, &address.resident, hub, account, None, operation).await
}
pub(crate) async fn resolve_here(
    node: &Arc<Node>,
    id: &ResidentId,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
    requesting: Option<&HubAccount>,
    operation: &Operation,
) -> Result<Target, Failure> {
    let gate = node.residents.management(id)?;
    let _one = crate::gate_waits::alone(node, id, &gate).await?;
    node.residents.active(id)?;
    super::heard(node, id).await?;
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::refused(words::RESIDENT))?;
    let previous = super::observed::load(node, id).await?;
    let scope = here(node, id, &about, previous.as_ref().map(|seen| &seen.account), hub, account, requesting)?;
    let preparation = if matches!(operation, Operation::Write { .. }) {
        Some(crate::placement::location::selection::resolve(node, id, &about.acting).await?)
    } else {
        None
    };
    Ok(Target::Here { resident: id.clone(), scope, preparation })
}
impl Target {
    pub(crate) fn check(&self, node: &Node, caller: &Caller) -> Result<(), Failure> {
        let Caller::Resident(address) = caller else {
            return Err(Failure::not_run(words::RESIDENT));
        };
        match self {
            Self::Here { resident, scope, preparation } => {
                if !node.is_me(&address.machine) || &address.resident != resident {
                    return Err(Failure::refused(words::RESIDENT));
                }
                node.residents.active(resident)?;
                scope.check(node, resident)?;
                if let Some(preparation) = preparation {
                    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::refused(words::RESIDENT))?;
                    preparation.check(node, &about.acting)?;
                }
            }
            Self::Remote { address: target, scope } => {
                if address != target || !crate::jobs::running_for(&node.world, &node.names(), target).contains(scope) {
                    return Err(Failure::refused(words::ROUTE));
                }
                let link = node.link_of(&scope.hub).ok_or_else(|| Failure::refused(words::ROUTE))?;
                let view = crate::authorization::current_view(&link)?;
                view.admits(&view.me, &scope.account, &target.machine, Purpose::Echo).map_err(Failure::refused)?;
                if !view.conversation_plans {
                    return Err(Failure::not_run(words::OLDER));
                }
            }
        }
        Ok(())
    }
    pub(crate) async fn execute(
        &self,
        node: &Arc<Node>,
        operation: Operation,
        of: Option<fastctx_types::plan::Of>,
    ) -> Result<fastctx_types::plan::Result, Failure> {
        match self {
            Self::Here { resident, scope, preparation } => {
                super::here_selected(node, resident, scope, preparation.as_ref(), operation, of).await
            }
            Self::Remote { address, scope } => {
                let link = node.link_of(&scope.hub).ok_or_else(|| Failure::refused(words::ROUTE))?;
                let call =
                    fastctx_types::remote::RemoteCall::Plan { resident: address.resident.clone(), operation, of };
                match crate::record_changes::transport::linked(
                    &link,
                    &address.machine,
                    &scope.account,
                    Purpose::Echo,
                    call,
                )
                .await?
                {
                    fastctx_types::remote::RemoteReply::Plan { result } => Ok(result),
                    _ => Err(Failure::new(words::REPLY)),
                }
            }
        }
    }
}
