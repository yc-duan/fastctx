//! Execute accepted original arguments with current authorization and the existing gates.
use super::failure::settled;
use super::{Answer, Asked, Resolved};
use crate::Node;
use fastctx_types::{
    Call, Reply,
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
};
use std::sync::Arc;

pub(super) async fn execute(node: &Arc<Node>, asked: Asked, resolved: Resolved) -> Result<Answer, Failure> {
    match asked {
        Asked::Local { peer, mut call } => {
            let reply = async {
                let caller = crate::town::caller::of(node, &peer)?;
                crate::admission::local::admit(node, &peer, &call)?;
                resolved.bind(&mut call)?;
                let town = match call.as_ref() {
                    Call::Page(call) => call.town(),
                    Call::Project(fastctx_types::ProjectCall::Edit { town, .. }) => town.as_ref(),
                    _ => None,
                };
                crate::server::named_town(node, &caller, town).await?;
                resolved.check(node, &caller)?;
                super::local_check(node, &caller, &call, &resolved)?;
                let reply = match *call {
                    Call::Page(call) => {
                        let Resolved::Page { identity, .. } = &resolved else {
                            return Err(Failure::not_run("the page has no selected identity"));
                        };
                        crate::pages::selected(node, &caller, &node.machine(), call, identity.as_ref()).await?
                    }
                    Call::Plan { operation, of, .. } => {
                        let Resolved::Plan(target) = &resolved else {
                            return Err(Failure::not_run("the plan has no selected coordinates"));
                        };
                        let result = target.execute(node, operation, of).await?;
                        Reply::Plan { result }
                    }
                    Call::PlanWrite { owner, expected, markdown, machine, .. } => {
                        let Resolved::PlanWrite { scope } = &resolved else {
                            return Err(Failure::not_run("the plan has no selected account"));
                        };
                        let link = node
                            .link_of(&scope.hub)
                            .ok_or_else(|| Failure::refused("the plan's selected network is unavailable"))?;
                        let account = scope.account.clone();
                        let whose = machine.map(|machine| fastctx_types::Address { machine, resident: owner.clone() });
                        let result = link
                            .plan_write(
                                &account,
                                owner,
                                expected,
                                markdown,
                                fastctx_types::plan::Author::User,
                                whose.as_ref(),
                            )
                            .await
                            .map_err(Failure::new)?;
                        Reply::Plan { result }
                    }
                    Call::Project(call) => {
                        if !matches!(call, fastctx_types::ProjectCall::Enter { .. }) {
                            crate::server::named_town(node, &caller, call.town()).await?;
                        }
                        let owner = match &resolved {
                            Resolved::ProjectNew { owner } => Some(owner.clone()),
                            _ => None,
                        };
                        crate::town::project::selected(node, &caller, call, owner).await?
                    }
                    Call::PrepareCopies { paths } => {
                        crate::replicas::sync::repair_paths(node, &paths).await.map_err(Failure::new)?;
                        Reply::Done
                    }
                    _ => return Err(Failure::not_run("this call is not a submitted record change")),
                };
                Ok(Box::new(reply))
            }
            .await;
            Ok(Answer::Local { reply: settled(reply), resolved })
        }
        Asked::Remote { scope, from, key, call } => {
            let reply = async {
                let link = node
                    .link_of(&scope.hub)
                    .ok_or_else(|| Failure::refused("the requesting network is unavailable"))?;
                if let Some(town) = crate::admission::town_of(&call)
                    && link.view().project_named(&scope.account, town).is_none()
                {
                    link.refresh().await.map_err(Failure::not_run)?;
                }
                let view = crate::authorization::current_view(&link)?;
                let member =
                    view.by_key(&key).ok_or_else(|| Failure::refused("the requesting machine left the network"))?;
                let caller = fastctx_net::link::Caller {
                    machine: member.name.clone(),
                    account: scope.account.clone(),
                    key,
                    names: member.was.clone(),
                    sent_as: from,
                    received: None,
                };
                let asker = crate::admission::admit(node, &link, &caller, &call)?;
                super::remote_check(node, &asker, &call)?;
                let reply = match *call {
                    RemoteCall::PageAction { source, action } => {
                        let result = crate::pages::actions::answer(node, &asker, source, action).await?;
                        RemoteReply::PageAction { result }
                    }
                    RemoteCall::Relay { made } => crate::devices::relay(node, &asker, made).await?,
                    RemoteCall::Plan { resident, operation, of } => {
                        let caller = crate::town::caller::Caller::Resident(fastctx_types::Address {
                            machine: node.machine(),
                            resident,
                        });
                        resolved.check(node, &caller)?;
                        let Resolved::Plan(target) = &resolved else {
                            return Err(Failure::not_run("the plan has no selected coordinates"));
                        };
                        let result = target.execute(node, operation, of).await?;
                        RemoteReply::Plan { result }
                    }
                    RemoteCall::EnterProject { resident, town } => {
                        let map = crate::town::enter::here(node, &resident, &town, Some(&scope)).await?;
                        RemoteReply::Entered { town, map: Box::new(map) }
                    }
                    RemoteCall::WorkingCopy { owner, placement, resource } => {
                        let location = crate::placement::prepare_copy(node, &asker, owner, placement, resource).await?;
                        RemoteReply::WorkingCopy { location }
                    }
                    _ => return Err(Failure::not_run("this remote call is not a submitted record change")),
                };
                Ok(Box::new(reply))
            }
            .await;
            Ok(Answer::Remote { reply: settled(reply), resolved })
        }
    }
}
