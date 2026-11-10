//! Asking fixes the requester's project and turn before its creation job is queued.
use crate::Node;
use fastctx_types::access::Acting;
use fastctx_types::collab::{Caller, Started};
use fastctx_types::ipc::Failure;
use fastctx_types::{Address, ResidentId, TownName};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

const FAMILY: crate::submitted::Family = crate::submitted::Family {
    name: "call/ask",
    at_once: 8,
    within: Duration::from_secs(300),
    stays: Duration::from_secs(240),
    again: false,
};
#[derive(Serialize, Deserialize)]
enum Asked {
    Route {
        asker: ResidentId,
        label: String,
        prompt: String,
        callee: Option<String>,
        login: Option<Caller>,
    },
    Here {
        requester: Address,
        turn: Option<String>,
        acting: Acting,
        town: Option<TownName>,
        label: String,
        prompt: String,
        started: Box<Started>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
struct Resolved {
    town: Option<TownName>,
    turn: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Answer {
    result: Result<(Address, Option<u32>), Failure>,
    resolved: Resolved,
}

pub(super) fn register(node: &Arc<Node>) {
    crate::submitted::register_resolved(node, FAMILY, |node: Arc<Node>, asked: Asked, resolved: Resolved| async move {
        let result = async {
            let (colleague, waiting) = match asked {
                Asked::Route { asker, label, prompt, callee, login } => {
                    check_route(&node, &asker, &resolved)?;
                    let requester = crate::colleagues::Requester {
                        address: Address { machine: node.machine(), resident: asker },
                        turn: resolved.turn.clone(),
                    };
                    let born = crate::colleagues::ask(
                        &node,
                        requester,
                        resolved.town.clone(),
                        label,
                        prompt,
                        callee,
                        login.as_ref(),
                    )
                    .await?;
                    (born.colleague, born.waiting)
                }
                Asked::Here { requester, acting, label, prompt, started, .. } => {
                    let requester = crate::colleagues::Requester { address: requester, turn: resolved.turn.clone() };
                    let (resident, waiting) = crate::colleagues::ask_here(
                        &node,
                        requester,
                        acting,
                        resolved.town.clone(),
                        label,
                        prompt,
                        *started,
                    )
                    .await?;
                    (Address { machine: node.machine(), resident }, waiting)
                }
            };
            Ok((colleague, waiting))
        }
        .await;
        Ok(Answer { result, resolved })
    });
}
fn check_route(node: &Node, asker: &ResidentId, resolved: &Resolved) -> Result<(), Failure> {
    node.residents.active(asker)?;
    if let Some(town) = &resolved.town {
        let caller =
            crate::town::caller::Caller::Resident(Address { machine: node.machine(), resident: asker.clone() });
        crate::town::access::check(node, &crate::around::acting_of(node, &caller), town)?;
    }
    Ok(())
}
fn unit(asker: &Address) -> String {
    let bytes = serde_json::to_vec(asker).expect("an address serializes");
    format!("ask/{}", fastctx_hands::files::content_hash(&bytes))
}

pub(crate) async fn ask(
    node: &Arc<Node>,
    asker: ResidentId,
    label: String,
    prompt: String,
    callee: Option<String>,
    login: Option<Caller>,
) -> Result<crate::colleagues::Born, Failure> {
    node.residents.active(&asker)?;
    crate::town::access::current(node, &asker)?;
    let town = node
        .residents
        .town_of(&asker)
        .ok_or_else(|| Failure::not_run(format!("no resident {asker} lives on {}", node.machine())))?;
    let turn = node.residents.now(&asker).and_then(|now| now.turn);
    let resolved = Resolved { town, turn };
    let unit = asker.to_string();
    let asked = Asked::Route { asker: asker.clone(), label, prompt, callee, login };
    let answer: Answer = crate::submitted::submit_resolved(node, &FAMILY, &unit, &asked, &resolved).await?;
    check_route(node, &asker, &answer.resolved)?;
    let (colleague, waiting) = answer.result?;
    Ok(crate::colleagues::Born { colleague, waiting })
}

pub(crate) async fn ask_here(
    node: &Arc<Node>,
    requester: crate::colleagues::Requester,
    acting: Acting,
    town: Option<TownName>,
    label: String,
    prompt: String,
    started: Started,
) -> Result<(ResidentId, Option<u32>), Failure> {
    if !node.is_me(&started.callee.login.machine) {
        return Err(Failure::not_run("the configured callee lives on another machine"));
    }
    crate::logins::workbench::may_start(node, &acting, "codex", started.callee.login.home.as_deref())?;
    let unit = unit(&requester.address);
    let resolved = Resolved { town: town.clone(), turn: requester.turn.clone() };
    let asked = Asked::Here {
        requester: requester.address,
        turn: requester.turn,
        acting: acting.clone(),
        town,
        label,
        prompt,
        started: Box::new(started),
    };
    let answer: Answer = crate::submitted::submit_resolved(node, &FAMILY, &unit, &asked, &resolved).await?;
    if let Some(town) = &answer.resolved.town {
        crate::town::access::check(node, &acting, town)?;
    }
    let (colleague, waiting) = answer.result?;
    Ok((colleague.resident, waiting))
}
