//! Coordinates selected at the entrance, independent of the original asking.
use crate::{Node, town::caller::Caller};
use fastctx_types::{Call, ProjectCall, TownName, access::HubAccount, ipc::Failure};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
pub(super) enum Resolved {
    Direct,
    Plan(crate::plan::selection::Target),
    /// The account as it was opened when the write was taken, in
    /// milliseconds: one made again under its name since is not it (AV07).
    PlanWrite {
        scope: HubAccount,
        #[serde(default)]
        opened: Option<i64>,
    },
    ProjectNew {
        owner: Option<HubAccount>,
    },
    ProjectEdit {
        town: TownName,
    },
}
impl Resolved {
    pub(super) async fn select(node: &Arc<Node>, caller: &Caller, call: &Call) -> Result<Self, Failure> {
        Ok(match call {
            Call::Plan { hub, account, operation, .. } => Self::Plan(
                crate::plan::selection::resolve(node, caller, hub.as_ref(), account.as_ref(), operation).await?,
            ),
            Call::PlanWrite { hub, account, .. } => {
                let (link, account) = crate::profile::scope(node, caller, hub.as_ref(), account.as_ref())?;
                let opened = link.view().account(&account).map(|found| found.opened.as_millisecond());
                Self::PlanWrite { scope: HubAccount { hub: link.membership().id, account }, opened }
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
            Self::Plan(target) => target.check(node, caller)?,
            Self::PlanWrite { scope, opened } => check_written_offline(node, caller, scope, *opened)?,
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
            (Self::ProjectEdit { town }, Call::Project(ProjectCall::Edit { town: selected, .. })) => {
                *selected = Some(town.clone())
            }
            _ => {}
        }
        Ok(())
    }
}
/// As `check_scope`, for a plan written while the hub is away: judged by the
/// view the hub gave last, the hub judging it again as it writes it (design
/// §20, 3-3: an edit made offline, then back).
fn check_written_offline(node: &Node, caller: &Caller, scope: &HubAccount, opened: Option<i64>) -> Result<(), Failure> {
    let link =
        node.link_of(&scope.hub).ok_or_else(|| Failure::refused("the selected account's network is unavailable"))?;
    let view = match crate::authorization::current_view(&link) {
        Ok(view) => view,
        Err(failure) if crate::authorization::view_pending(&failure) => link.view(),
        Err(failure) => return Err(failure),
    };
    if !node.serves(&crate::around::acting_of(node, caller), &scope.hub, &scope.account)
        || view.account(&scope.account).is_none_or(|account| account.locked)
    {
        return Err(Failure::refused("the caller no longer serves the selected account"));
    }
    if opened.is_some_and(|opened| {
        view.account(&scope.account).is_some_and(|account| account.opened.as_millisecond() != opened)
    }) {
        return Err(Failure::refused("the account the plan was written for was deleted and made again since"));
    }
    Ok(())
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
