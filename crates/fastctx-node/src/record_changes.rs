//! Durable entry points for record changes and working-copy preparation.

mod copies;
mod failure;
mod plans;
mod projects;
mod replies;
mod resolved;
pub(crate) mod transport;
mod work;

use crate::{
    Node,
    submitted::{self, Family},
};
use fastctx_types::{
    Call, MachineName, Reply,
    access::HubAccount,
    ipc::{Failure, Peer},
    remote::{RemoteCall, RemoteReply},
};
use resolved::Resolved;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

const fn family(name: &'static str) -> Family {
    Family { name, at_once: 8, within: Duration::from_secs(1800), stays: Duration::from_secs(240), again: false }
}
const PLANS: Family = family("call/plans");
/// A plan written, done again after a stop that cut it short: it waits for
/// the hub when written offline, and asked again under the same base it is
/// at worst refused as written over (QQ15); apart from the other plan calls,
/// which do not wait behind it (QQ16).
const PLAN_WRITES: Family = Family { again: true, ..family("call/plan-writes") };
const PROJECTS: Family = family("call/projects");
const COPIES: Family = family("call/copies");
/// Calls of pages an earlier release accepted: pages are no more (beta.18),
/// and each is answered so, never left waiting (codex-e7b1552a R5).
const RETIRED: [Family; 3] = [family("call/pages"), family("call/page-act"), family("call/page-execute")];

// Only the explicitly matched calls below enter these recordings. In particular,
// authentication and login calls, whose arguments contain secrets, never do.
#[derive(Serialize, Deserialize)]
enum Asked {
    Local { peer: Peer, call: Box<Call> },
    Remote { scope: HubAccount, from: MachineName, key: fastctx_net::sign::PublicKey, call: Box<RemoteCall> },
}
#[derive(Serialize, Deserialize)]
enum Answer {
    Local { reply: Result<Box<Reply>, Failure>, resolved: Resolved },
    Remote { reply: Result<Box<RemoteReply>, Failure>, resolved: Resolved },
}

pub(crate) fn register(node: &Arc<Node>) {
    for family in [PLANS, PLAN_WRITES, PROJECTS, COPIES] {
        submitted::register_resolved(node, family, |node, asked, resolved| async move {
            work::execute(&node, asked, resolved).await
        });
    }
    for family in RETIRED {
        submitted::register_resolved(
            node,
            family,
            |_node, _asked: serde_json::Value, _resolved: serde_json::Value| async move {
                Err::<(), _>(Failure::new("pages were removed in Fastctx beta.18; this page's call was not carried out"))
            },
        );
    }
}

pub(crate) fn handles(call: &Call) -> bool {
    match call {
        Call::Plan { operation, of, .. } => of.is_none() || !matches!(operation, fastctx_types::plan::Operation::Read),
        Call::PlanWrite { .. } | Call::PrepareCopies { .. } => true,
        Call::Project(
            fastctx_types::ProjectCall::New { .. }
            | fastctx_types::ProjectCall::Edit { .. }
            | fastctx_types::ProjectCall::Enter { .. },
        ) => true,
        _ => false,
    }
}

pub(crate) fn handles_remote(call: &RemoteCall) -> bool {
    match call {
        RemoteCall::EnterProject { .. } | RemoteCall::WorkingCopy { .. } => true,
        RemoteCall::Plan { operation, of, .. } => {
            of.is_none() || !matches!(operation, fastctx_types::plan::Operation::Read)
        }
        _ => false,
    }
}

pub(crate) async fn local(node: &Arc<Node>, peer: &Peer, call: Call) -> Result<Reply, Failure> {
    let caller = crate::town::caller::of(node, peer)?;
    crate::admission::local::admit(node, peer, &call)?;
    match &call {
        Call::Plan { .. } | Call::PlanWrite { .. } => plans::parameters(node, &caller, &call)?,
        Call::Project(call) => projects::parameters(node, &caller, call)?,
        Call::PrepareCopies { .. } => node.authorize_here(&crate::around::acting_of(node, &caller))?,
        _ => {}
    }
    let family = local_family(&call)?;
    let asked = Asked::Local { peer: peer.clone(), call: Box::new(call) };
    let answer = submitted::submit_resolving(node, &family, &asked, || async {
        let Asked::Local { call, .. } = &asked else { unreachable!("local recording") };
        let resolved = Resolved::select(node, &caller, call).await?;
        let (_, unit) = local_check(node, &caller, call, &resolved)?;
        Ok((unit, resolved))
    })
    .await?;
    match answer {
        Answer::Local { reply, resolved } => {
            let caller = crate::town::caller::of(node, peer)?;
            let Asked::Local { call, .. } = &asked else { unreachable!("local recording") };
            crate::admission::local::admit(node, peer, call)?;
            local_check(node, &caller, call, &resolved)?;
            resolved.check(node, &caller)?;
            let mut reply = reply?;
            replies::local(node, &caller, call, &mut reply, &resolved)?;
            Ok(*reply)
        }
        Answer::Remote { .. } => Err(Failure::new("the record change returned a remote answer")),
    }
}

pub(crate) async fn remote(
    node: &Arc<Node>,
    scope: HubAccount,
    caller: &fastctx_net::link::Caller,
    call: RemoteCall,
) -> Result<RemoteReply, Failure> {
    let link = node.link_of(&scope.hub).ok_or_else(|| Failure::refused("the requesting network is unavailable"))?;
    let asker = crate::admission::admit_for_submission(node, &link, caller, &call)?;
    remote_parameters(node, &asker, &call)?;
    let family = remote_family(&call)?;
    let asked = Asked::Remote { scope, from: caller.machine.clone(), key: caller.key, call: Box::new(call) };
    let answer = submitted::submit_resolving(node, &family, &asked, || async {
        let Asked::Remote { scope, call, .. } = &asked else { unreachable!("remote recording") };
        let (_, unit) = remote_check(node, &asker, call)?;
        let resolved = match call.as_ref() {
            RemoteCall::Plan { resident, operation, .. } => Resolved::Plan(
                crate::plan::selection::resolve_here(node, resident, None, None, Some(scope), operation).await?,
            ),
            _ => Resolved::Direct,
        };
        Ok((unit, resolved))
    })
    .await?;
    match answer {
        Answer::Remote { reply, resolved } => {
            let Asked::Remote { call, .. } = &asked else { unreachable!("remote recording") };
            let asker = crate::admission::admit(node, &link, caller, call)?;
            if let RemoteCall::Plan { resident, .. } = call.as_ref() {
                resolved.check(
                    node,
                    &crate::town::caller::Caller::Resident(fastctx_types::Address {
                        machine: node.machine(),
                        resident: resident.clone(),
                    }),
                )?;
            }
            let mut reply = reply?;
            replies::remote(node, &asker, call, &mut reply)?;
            Ok(*reply)
        }
        Answer::Local { .. } => Err(Failure::new("the record change returned a local answer")),
    }
}

fn local_family(call: &Call) -> Result<Family, Failure> {
    match call {
        Call::Plan { .. } => Ok(PLANS),
        Call::PlanWrite { .. } => Ok(PLAN_WRITES),
        Call::Project(_) => Ok(PROJECTS),
        Call::PrepareCopies { .. } => Ok(COPIES),
        _ => Err(Failure::not_run("this call is not a submitted record change")),
    }
}

fn remote_family(call: &RemoteCall) -> Result<Family, Failure> {
    match call {
        RemoteCall::Plan { .. } => Ok(PLANS),
        RemoteCall::EnterProject { .. } => Ok(PROJECTS),
        RemoteCall::WorkingCopy { .. } => Ok(COPIES),
        _ => Err(Failure::not_run("this remote call is not a submitted record change")),
    }
}

fn remote_parameters(node: &Arc<Node>, asker: &crate::admission::Asker, call: &RemoteCall) -> Result<(), Failure> {
    match call {
        RemoteCall::Plan { resident, .. } | RemoteCall::EnterProject { resident, .. } => {
            node.residents.active(resident)
        }
        RemoteCall::WorkingCopy { placement, resource, .. } => copies::parameters(node, asker, *placement, resource),
        _ => Err(Failure::not_run("this remote call is not a submitted record change")),
    }
}

fn local_check(
    node: &Arc<Node>,
    caller: &crate::town::caller::Caller,
    call: &Call,
    resolved: &Resolved,
) -> Result<(Family, String), Failure> {
    match call {
        Call::Plan { .. } | Call::PlanWrite { .. } => {
            plans::check(node, caller, call, resolved).map(|unit| (PLANS, unit))
        }
        Call::Project(call) => projects::check(node, caller, call, resolved).map(|unit| (PROJECTS, unit)),
        Call::PrepareCopies { paths } => copies::paths_check(node, caller, paths).map(|unit| (COPIES, unit)),
        _ => Err(Failure::not_run("this call is not a submitted record change")),
    }
}

fn remote_check(
    node: &Arc<Node>,
    asker: &crate::admission::Asker,
    call: &RemoteCall,
) -> Result<(Family, String), Failure> {
    match call {
        RemoteCall::Plan { resident, .. } => {
            node.residents.active(resident)?;
            unit(&("conversation", resident)).map(|unit| (PLANS, unit))
        }
        RemoteCall::EnterProject { resident, .. } => {
            node.residents.active(resident)?;
            unit(&("conversation", resident)).map(|unit| (PROJECTS, unit))
        }
        RemoteCall::WorkingCopy { owner, placement, resource } => {
            copies::check(node, asker, owner, *placement, resource).map(|unit| (COPIES, unit))
        }
        _ => Err(Failure::not_run("this remote call is not a submitted record change")),
    }
}

fn unit(value: &impl Serialize) -> Result<String, Failure> {
    let bytes = serde_json::to_vec(value).map_err(|error| Failure::new(error.to_string()))?;
    Ok(fastctx_hands::files::content_hash(&bytes))
}
