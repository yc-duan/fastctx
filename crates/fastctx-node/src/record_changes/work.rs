//! Execute accepted original arguments with current authorization and the existing gates.
use super::{Answer, Asked};
use crate::Node;
use fastctx_types::{
    Call, Reply,
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
};
use std::sync::Arc;

pub(super) async fn execute(node: &Arc<Node>, asked: Asked) -> Result<Answer, Failure> {
    match asked {
        Asked::Local { peer, call } => {
            let caller = crate::town::caller::of(node, &peer)?;
            crate::admission::local::admit(node, &peer, &call)?;
            super::local_check(node, &caller, &call)?;
            let reply = match *call {
                Call::Page(call) => {
                    crate::server::named_town(node, &caller, call.town()).await?;
                    crate::pages::call(node, &caller, &node.machine(), call).await?
                }
                Call::Plan { hub, account, operation, of } => {
                    let result = crate::plan::call(node, &caller, hub, account, operation, of).await?;
                    Reply::Plan { result }
                }
                Call::PlanWrite { hub, account, owner, expected, markdown, machine } => {
                    let (link, account) = crate::profile::scope(node, &caller, hub.as_ref(), account.as_ref())?;
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
                    crate::town::project::call(node, &caller, call).await?
                }
                Call::PrepareCopies { paths } => {
                    crate::replicas::sync::repair_paths(node, &paths).await.map_err(Failure::new)?;
                    Reply::Done
                }
                _ => return Err(Failure::not_run("this call is not a submitted record change")),
            };
            Ok(Answer::Local(Box::new(reply)))
        }
        Asked::Remote { scope, from, key, call } => {
            let link =
                node.link_of(&scope.hub).ok_or_else(|| Failure::refused("the requesting network is unavailable"))?;
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
                    let result = crate::plan::here(node, &resident, None, None, Some(&scope), operation, of).await?;
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
            Ok(Answer::Remote(Box::new(reply)))
        }
    }
}
