//! Resident commands execute at the goal's home, preserving their actual author.

use super::{intents, operations, scope, words};
use crate::Node;
use fastctx_types::access::{Acting, HubAccount, Purpose};
use fastctx_types::goals::{AgentRequest, Author, ColleagueControl, Owner, Request, Updated, UserChange};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{Address, ResidentId};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Find the colleague's account among those the caller may use. A caller's own
/// goal can belong to a different account and must not select this one for it.
pub(crate) async fn target(
    node: &Arc<Node>,
    actor: &Address,
    colleague: &ResidentId,
    selected: Option<HubAccount>,
) -> Result<(Address, HubAccount), Failure> {
    let acting = crate::around::acting_of(node, &crate::town::caller::Caller::Resident(actor.clone()));
    let allowed: BTreeSet<_> = node
        .links()
        .into_iter()
        .flat_map(|link| {
            node.accounts_on(&link, &acting)
                .into_iter()
                .map(move |account| HubAccount { hub: link.membership().id.clone(), account })
        })
        .collect();
    let choices = if let Some(selected) = selected {
        if !allowed.contains(&selected) {
            return Err(Failure::refused(words::ACCOUNT_MISSING));
        }
        vec![selected]
    } else {
        allowed.into_iter().collect()
    };
    let mut found = Vec::new();
    for account in choices {
        let (view, _) = operations::by_id(node, colleague, &account).await?;
        if let Some(goal) = view.goal
            && matches!(&goal.owner, Owner::Conversation { address } if super::same(node, address, actor))
        {
            found.push((goal.resident, account));
        }
    }
    match found.len() {
        1 => Ok(found.pop().expect("one matching colleague goal")),
        0 => Err(Failure::refused(words::OWNER_ONLY)),
        _ => Err(Failure::not_run(words::ACCOUNT_AMBIGUOUS)),
    }
}

pub(crate) async fn route(
    node: &Arc<Node>,
    target: Address,
    actor: Address,
    account: HubAccount,
    turn: Option<String>,
    request: AgentRequest,
) -> Result<Updated, Failure> {
    if node.is_me(&target.machine) {
        return here(node, actor, account, turn, request).await;
    }
    let link = node.link_of(&account.hub).ok_or_else(|| Failure::not_run(words::ACCOUNT_MISSING))?;
    if !link.view().goal_commands {
        return Err(Failure::not_run(words::OLDER_COMMAND_HUB));
    }
    let acting = Acting::one(account.hub.clone(), account.account.clone());
    let machine = node.written(&link, &target.machine);
    let facts = node.call(&acting, &machine, Purpose::Look, RemoteCall::Facts).await.map_err(failure)??;
    if !matches!(facts, RemoteReply::Facts(facts) if facts.goal_runtime && facts.goal_commands) {
        return Err(Failure::not_run(words::OLDER_COMMAND_NODE));
    }
    let call = RemoteCall::GoalCommand { actor, turn, request };
    let answer = node.call(&acting, &machine, call.purpose(), call).await.map_err(failure)??;
    match answer {
        RemoteReply::Goal { updated } => Ok(*updated),
        _ => Err(Failure::new(words::EXPECTED_GOAL)),
    }
}

/// Invoked after caller/job validation locally, or account and computer admission remotely.
pub(crate) async fn here(
    node: &Arc<Node>,
    mut actor: Address,
    account: HubAccount,
    turn: Option<String>,
    request: AgentRequest,
) -> Result<Updated, Failure> {
    let resident = request.resident(&actor).clone();
    if !matches!(request, AgentRequest::ControlColleague { .. }) && !node.is_me(&actor.machine) {
        return Err(Failure::refused(words::CONVERSATION_CHANGED));
    }
    if node.is_me(&actor.machine) {
        let (about, _) =
            node.residents.about(&actor.resident).ok_or_else(|| crate::forget::gone(node, &actor.resident))?;
        if !node.serves(&about.acting, &account.hub, &account.account) {
            return Err(Failure::refused(words::ACCOUNT_MISSING));
        }
        actor.machine = node.machine();
    }
    let account = scope::resolve(node, &resident, Some(&account))?;
    let address = Address { machine: node.machine(), resident: resident.clone() };
    if matches!(request, AgentRequest::Read) {
        if !node.goals.records.available() {
            return Err(Failure::not_run(words::RECORDS_MISSING));
        }
        return operations::read(node, &address, &account).await.map(|view| Updated::Goal { view: Box::new(view) });
    }
    super::colleagues::ready(node)?;
    let author = Author::Resident { address: actor };
    match request {
        AgentRequest::Mark { operation, mark } => {
            operations::call(node, address, account, author, turn, Request::Mark { operation, mark }).await
        }
        AgentRequest::ControlColleague { operation, control, .. } => {
            let gate = crate::control_queue::gate(node, &resident);
            let _action = gate.lock().await;
            let _place = node.places.using(resident.as_str()).await;
            let expected = match intents::read(&node.world.conversation(&resident), &operation).map_err(failure)? {
                Some(intent) => intent.expected,
                None => operations::read(node, &address, &account).await?.version,
            };
            let change = match control {
                ColleagueControl::Pause => UserChange::Pause,
                ColleagueControl::Resume => UserChange::Resume,
            };
            operations::call_held(node, address, account, author, None, Request::Change { operation, expected, change })
                .await
        }
        AgentRequest::Read => unreachable!(),
    }
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
