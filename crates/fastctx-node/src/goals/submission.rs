//! Goal mutations keep their resolved caller facts while their work runs after submission.
use crate::{
    Node,
    submitted::{self, Family},
};
use fastctx_types::{
    Address, MachineName, ResidentId,
    access::HubAccount,
    goals::{AgentRequest, Request, Updated},
    ipc::Failure,
};
use std::{sync::Arc, time::Duration};

const FAMILY: Family = Family {
    name: "call/goal",
    at_once: 8,
    within: Duration::from_secs(30 * 60),
    stays: Duration::from_secs(240),
    // Preparation can archive a goal before its durable intent exists.
    again: false,
};

#[derive(serde::Serialize, serde::Deserialize)]
enum Asked {
    User { resident: ResidentId, account: HubAccount, machine: MachineName, request: Request },
    Command { actor: Address, account: HubAccount, turn: Option<String>, request: AgentRequest },
}

pub(crate) fn register(node: &Arc<Node>) {
    submitted::register(node, FAMILY, |node: Arc<Node>, asked: Asked| async move {
        match asked {
            Asked::User { resident, account, machine, request } => {
                super::calls::user_work(&node, resident, account, machine, request).await
            }
            Asked::Command { actor, account, turn, request } => {
                super::commands::work(&node, actor, account, turn, request).await
            }
        }
    });
}

pub(super) async fn user(
    node: &Arc<Node>,
    resident: ResidentId,
    account: HubAccount,
    machine: MachineName,
    request: Request,
) -> Result<Updated, Failure> {
    let unit = resident.to_string();
    submitted::submit(node, &FAMILY, &unit, &Asked::User { resident, account, machine, request }).await
}

pub(super) async fn command(
    node: &Arc<Node>,
    actor: Address,
    account: HubAccount,
    turn: Option<String>,
    request: AgentRequest,
) -> Result<Updated, Failure> {
    let unit = request.resident(&actor).to_string();
    submitted::submit(node, &FAMILY, &unit, &Asked::Command { actor, account, turn, request }).await
}
