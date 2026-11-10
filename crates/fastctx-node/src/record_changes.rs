//! Durable entry points for record changes and working-copy preparation.

mod copies;
mod pages;
mod plans;
mod projects;
mod replies;
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
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

const fn family(name: &'static str) -> Family {
    Family { name, at_once: 8, within: Duration::from_secs(1800), stays: Duration::from_secs(240), again: false }
}
const PAGES: Family = family("call/pages");
const PAGE_ACT: Family = family("call/page-act");
const PAGE_EXECUTE: Family = family("call/page-execute");
const PLANS: Family = family("call/plans");
const PROJECTS: Family = family("call/projects");
const COPIES: Family = family("call/copies");

// Only the explicitly matched calls below enter these recordings. In particular,
// authentication and login calls, whose arguments contain secrets, never do.
#[derive(Serialize, Deserialize)]
enum Asked {
    Local { peer: Peer, call: Box<Call> },
    Remote {
        scope: HubAccount,
        from: MachineName,
        key: fastctx_net::sign::PublicKey,
        call: Box<RemoteCall>,
    },
}
#[derive(Serialize, Deserialize)]
enum Answer {
    Local(Box<Reply>),
    Remote(Box<RemoteReply>),
}

pub(crate) fn register(node: &Arc<Node>) {
    for family in [PAGES, PAGE_ACT, PAGE_EXECUTE, PLANS, PROJECTS, COPIES] {
        submitted::register(node, family, |node, asked| async move { work::execute(&node, asked).await });
    }
}

pub(crate) fn handles(call: &Call) -> bool {
    match call {
        Call::Page(call) => pages::explicit(call),
        Call::Plan { hub: Some(_), account: Some(_), operation, of } => {
            of.is_none() || !matches!(operation, fastctx_types::plan::Operation::Read)
        }
        Call::PlanWrite { hub: Some(_), account: Some(_), .. } | Call::PrepareCopies { .. } => true,
        Call::Project(
            fastctx_types::ProjectCall::New { hub: Some(_), account: Some(_), .. }
            | fastctx_types::ProjectCall::Edit { town: Some(_), .. }
            | fastctx_types::ProjectCall::Enter { .. },
        ) => true,
        _ => false,
    }
}

pub(crate) fn handles_remote(call: &RemoteCall) -> bool {
    match call {
        RemoteCall::PageAction { .. } | RemoteCall::EnterProject { .. } | RemoteCall::WorkingCopy { .. } => true,
        RemoteCall::Plan { operation, of, .. } => {
            of.is_none() || !matches!(operation, fastctx_types::plan::Operation::Read)
        }
        RemoteCall::Relay { .. } => call.response() == fastctx_types::contract::Response::Submit,
        _ => false,
    }
}

pub(crate) async fn local(node: &Arc<Node>, peer: &Peer, call: Call) -> Result<Reply, Failure> {
    let caller = crate::town::caller::of(node, peer)?;
    crate::admission::local::admit(node, peer, &call)?;
    let (family, unit) = local_check(node, &caller, &call)?;
    let asked = Asked::Local { peer: peer.clone(), call: Box::new(call) };
    match submitted::submit(node, &family, &unit, &asked).await? {
        Answer::Local(mut reply) => {
            let caller = crate::town::caller::of(node, peer)?;
            let Asked::Local { call, .. } = &asked else { unreachable!("local recording") };
            crate::admission::local::admit(node, peer, call)?;
            local_check(node, &caller, call)?;
            replies::local(node, &caller, call, &mut reply)?;
            Ok(*reply)
        }
        Answer::Remote(_) => Err(Failure::new("the record change returned a remote answer")),
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
    let (family, unit) = remote_check(node, &asker, &call)?;
    let asked = Asked::Remote { scope, from: caller.machine.clone(), key: caller.key, call: Box::new(call) };
    match submitted::submit(node, &family, &unit, &asked).await? {
        Answer::Remote(mut reply) => {
            let Asked::Remote { call, .. } = &asked else { unreachable!("remote recording") };
            let asker = crate::admission::admit(node, &link, caller, call)?;
            replies::remote(node, &asker, call, &mut reply)?;
            Ok(*reply)
        }
        Answer::Local(_) => Err(Failure::new("the record change returned a local answer")),
    }
}

fn local_check(
    node: &Arc<Node>,
    caller: &crate::town::caller::Caller,
    call: &Call,
) -> Result<(Family, String), Failure> {
    match call {
        Call::Page(call) if pages::changes(call) => {
            let family = if matches!(call, fastctx_types::pages::PageCall::Act { .. }) { PAGE_ACT } else { PAGES };
            pages::check(node, caller, &node.machine(), call).map(|unit| (family, unit))
        }
        Call::Plan { .. } | Call::PlanWrite { .. } => plans::check(node, caller, call).map(|unit| (PLANS, unit)),
        Call::Project(call) => projects::check(node, caller, call).map(|unit| (PROJECTS, unit)),
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
        RemoteCall::PageAction { .. } => {
            pages::remote_check(node, asker, call).map(|unit| (PAGE_EXECUTE, unit))
        }
        RemoteCall::Relay { made } => {
            let family = if made.get("do").and_then(serde_json::Value::as_str) == Some("act") {
                PAGE_ACT
            } else {
                PAGES
            };
            pages::remote_check(node, asker, call).map(|unit| (family, unit))
        }
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

fn town(
    node: &Node,
    caller: &crate::town::caller::Caller,
    given: Option<&fastctx_types::TownName>,
) -> Result<fastctx_types::TownName, Failure> {
    // An explicit project's authorization may need the worker's refresh. Only
    // resolve the target here; the existing handler checks access before I/O.
    match given.cloned().or_else(|| caller.town(node)) {
        Some(town) => Ok(town),
        None => caller.town_or(node, None),
    }
}

fn unit(value: &impl Serialize) -> Result<String, Failure> {
    let bytes = serde_json::to_vec(value).map_err(|error| Failure::new(error.to_string()))?;
    Ok(fastctx_hands::files::content_hash(&bytes))
}
