//! Coordinates selected at the entrance, independent of the original asking.
use crate::{Node, town::caller::Caller};
use fastctx_types::{
    Call, ProjectCall, TownName,
    access::HubAccount,
    ipc::Failure,
    pages::{PageCall, PageIdentity},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
pub(super) enum Resolved {
    Direct,
    Page { town: TownName, identity: Option<PageIdentity> },
    Plan(crate::plan::selection::Target),
    PlanWrite { scope: HubAccount },
    ProjectNew { owner: Option<HubAccount> },
    ProjectEdit { town: TownName },
}
impl Resolved {
    pub(super) async fn select(node: &Arc<Node>, caller: &Caller, call: &Call) -> Result<Self, Failure> {
        Ok(match call {
            Call::Page(call) => {
                let town = match call.town() {
                    Some(town) => town.clone(),
                    None => caller.town_or(node, None)?,
                };
                let identity = if matches!(call, PageCall::Consent { .. } | PageCall::Act { .. }) {
                    if call.town().is_some() {
                        crate::server::named_town(node, caller, Some(&town)).await?;
                    }
                    if matches!(call, PageCall::Consent { .. }) {
                        crate::pages::refresh(node, &town).await?;
                    }
                    Some(crate::pages::identity::resolve(node, caller, &node.machine(), &town, call.account())?)
                } else {
                    None
                };
                Self::Page { town, identity }
            }
            Call::Plan { hub, account, operation, .. } => Self::Plan(
                crate::plan::selection::resolve(node, caller, hub.as_ref(), account.as_ref(), operation).await?,
            ),
            Call::PlanWrite { hub, account, .. } => {
                let (link, account) = crate::profile::scope(node, caller, hub.as_ref(), account.as_ref())?;
                Self::PlanWrite { scope: HubAccount { hub: link.membership().id, account } }
            }
            Call::Project(ProjectCall::New { account, hub, .. }) => {
                Self::ProjectNew { owner: crate::town::project::owner(node, caller, account.clone(), hub.clone())? }
            }
            Call::Project(ProjectCall::Edit { town, .. }) => Self::ProjectEdit {
                town: match town {
                    Some(town) => town.clone(),
                    None => caller.town_or(node, None)?,
                },
            },
            _ => Self::Direct,
        })
    }

    pub(super) fn check(&self, node: &Node, caller: &Caller) -> Result<(), Failure> {
        match self {
            Self::Page { town, identity } => {
                crate::town::access::check(node, &crate::around::acting_of(node, caller), town)?;
                if let Some(identity) = identity {
                    crate::pages::identity::check(node, caller, &node.machine(), town, identity)?;
                }
            }
            Self::Plan(target) => target.check(node, caller)?,
            Self::PlanWrite { scope } => check_scope(node, caller, scope)?,
            Self::ProjectNew { owner: Some(owner) } => check_scope(node, caller, owner)?,
            Self::ProjectNew { owner: None } => {}
            Self::ProjectEdit { town } => {
                crate::town::access::check(node, &crate::around::acting_of(node, caller), town)?
            }
            Self::Direct => {}
        }
        Ok(())
    }

    pub(super) fn bind(&self, call: &mut Call) -> Result<(), Failure> {
        match (self, call) {
            (Self::Page { town, identity }, Call::Page(call)) => {
                match call {
                    PageCall::Publish { town: selected, .. }
                    | PageCall::Withdraw { town: selected, .. }
                    | PageCall::Restore { town: selected, .. }
                    | PageCall::Consent { town: selected, .. }
                    | PageCall::Act { town: selected, .. } => *selected = Some(town.clone()),
                    _ => return Err(Failure::not_run("the resolved page request is not a change")),
                }
                if let Some(PageIdentity::Account { account, .. }) = identity {
                    call.for_account(account.clone());
                }
            }
            (Self::ProjectEdit { town }, Call::Project(ProjectCall::Edit { town: selected, .. })) => {
                *selected = Some(town.clone())
            }
            _ => {}
        }
        Ok(())
    }
}
pub(super) fn check_scope(node: &Node, caller: &Caller, scope: &HubAccount) -> Result<(), Failure> {
    let link =
        node.link_of(&scope.hub).ok_or_else(|| Failure::refused("the selected account's network is unavailable"))?;
    let view = crate::authorization::current_view(&link)?;
    if !node.serves(&crate::around::acting_of(node, caller), &scope.hub, &scope.account)
        || view.account(&scope.account).is_none_or(|account| account.locked)
    {
        return Err(Failure::refused("the caller no longer serves the selected account"));
    }
    Ok(())
}
