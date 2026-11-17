//! Lifecycle and home work have separate queues, so a stop never waits behind a queued wake.
use crate::{
    Node,
    submitted::{self, Family},
    town::caller::Caller,
};
use fastctx_types::{
    Address, MachineName,
    access::HubAccount,
    ipc::Failure,
    sentinel_control::{HomeReply, HomeRequest, Reply, Request},
};
use std::{sync::Arc, time::Duration};

const CREATE: Family = Family {
    name: "call/sentinel-create",
    at_once: 8,
    within: Duration::from_secs(15 * 60),
    stays: Duration::from_secs(240),
    // The definition receives a random ID before its first wake.
    again: false,
};
const STOP: Family = Family {
    name: "call/sentinel-stop",
    at_once: 32,
    within: Duration::from_secs(15 * 60),
    // Shared stop calls have a 60-second control deadline.
    stays: Duration::from_secs(40),
    // A repeated deletion does not retain the first stop's confirmation.
    again: false,
};
const BEGIN: Family = Family {
    name: "call/sentinel-begin",
    at_once: 8,
    within: Duration::from_secs(30 * 60),
    // The lifecycle caller waits 180 seconds for a first wake.
    stays: Duration::from_secs(150),
    // Replaying an interrupted wake can spend another model request.
    again: false,
};
const JUDGE: Family = Family {
    name: "call/sentinel-judge",
    at_once: 8,
    within: Duration::from_secs(30 * 60),
    // The goal's transport waits 240 seconds for a judgment.
    stays: Duration::from_secs(210),
    // A receipt cannot prove an interrupted provider request was not charged.
    again: false,
};
const WITHDRAW: Family = Family {
    name: "call/sentinel-withdraw",
    at_once: 32,
    within: Duration::from_secs(15 * 60),
    stays: Duration::from_secs(40),
    // Reconciliation can itself wake a still-active assignment.
    again: false,
};

#[derive(serde::Serialize, serde::Deserialize)]
struct Control {
    scope: HubAccount,
    resident: Option<Address>,
    request: Request,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Home {
    scope: HubAccount,
    from: MachineName,
    request: HomeRequest,
}

pub(super) fn register(node: &Arc<Node>) {
    for family in [CREATE, STOP] {
        submitted::register(node, family, |node: Arc<Node>, asked: Control| async move {
            let link = node
                .link_of(&asked.scope.hub)
                .ok_or_else(|| Failure::not_run("the sentinel account's network is not connected"))?;
            let caller = asked.resident.map_or(Caller::User, Caller::Resident);
            super::super::control::work(&node, &caller, &link, &asked.scope.account, asked.request).await
        });
    }
    for family in [BEGIN, JUDGE, WITHDRAW] {
        submitted::register(node, family, |node: Arc<Node>, asked: Home| async move {
            super::super::transport::home_work(&node, &asked.scope, &asked.from, &asked.request)
                .await
                .map_err(Failure::new)
        });
    }
}

pub(crate) async fn control(
    node: &Arc<Node>,
    caller: &Caller,
    scope: HubAccount,
    request: Request,
) -> Result<Reply, Failure> {
    let resident = match caller {
        Caller::User => None,
        Caller::Resident(address) => Some(address.clone()),
        _ => return Err(Failure::refused(fastctx_types::sentinel_words::NEED_CONVERSATION)),
    };
    let (family, unit) = match &request {
        Request::Create { .. } => (
            CREATE,
            resident
                .as_ref()
                .ok_or_else(|| Failure::refused(fastctx_types::sentinel_words::NEED_CONVERSATION))?
                .resident
                .to_string(),
        ),
        Request::Stop { id, .. } => (
            STOP,
            resident
                .as_ref()
                .map_or_else(|| super::scoped_unit("sentinel", &(&scope, id)), |owner| owner.resident.to_string()),
        ),
        Request::Conversation { owner, .. } => (STOP, owner.resident.to_string()),
        _ => return Err(Failure::not_run("this sentinel request is not submitted work")),
    };
    submitted::submit(node, &family, &unit, &Control { scope, resident, request }).await
}

pub(crate) async fn home(
    node: &Arc<Node>,
    scope: &HubAccount,
    from: &MachineName,
    request: &HomeRequest,
) -> Result<HomeReply, Failure> {
    let (family, unit) = match request {
        HomeRequest::Begin { owner, .. } => (BEGIN, owner.resident.to_string()),
        HomeRequest::Judge { request } => (JUDGE, request.owner.resident.to_string()),
        HomeRequest::Withdraw { id } => (WITHDRAW, super::scoped_unit("sentinel", &(scope, id))),
        HomeRequest::Conversation { owner, .. } => (WITHDRAW, owner.resident.to_string()),
        _ => return Err(Failure::not_run("this sentinel home request is not submitted work")),
    };
    submitted::submit(
        node,
        &family,
        &unit,
        &Home { scope: scope.clone(), from: from.clone(), request: request.clone() },
    )
    .await
}
