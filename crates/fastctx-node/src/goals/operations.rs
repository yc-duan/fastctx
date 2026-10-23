//! Serialize a local decision, compare it at the account, then apply the authoritative context.

use super::{
    intents::{self, Intent, Phase},
    records::{Exchanged, Record},
    state, words,
};
use crate::Node;
use fastctx_types::access::HubAccount;
use fastctx_types::goals::{Author, Goal, Owner, Request, Updated, UserChange, View};
use fastctx_types::ipc::Failure;
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;
use std::sync::Arc;
use std::time::Duration;

pub(crate) const KIND: &str = "goal";

pub(crate) async fn read(node: &Node, resident: &Address, account: &HubAccount) -> Result<View, Failure> {
    let record = bounded(node.goals.records.read(account, KIND, resident.resident.as_str())).await?;
    view(node, resident, account, record)
}

fn view(node: &Node, resident: &Address, account: &HubAccount, record: Option<Record>) -> Result<View, Failure> {
    let version = record.as_ref().map(|record| record.version);
    let goal: Option<Goal> = record
        .and_then(|record| record.body)
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| Failure::new(format!("reading the goal record: {error}")))?;
    if let Some(goal) = &goal {
        if goal.account != *account {
            return Err(Failure::new(words::ACCOUNT_CHANGED));
        }
        if !super::same(node, &goal.resident, resident) {
            return Err(Failure::new(words::CONVERSATION_CHANGED));
        }
    }
    let mut view = View { account: account.clone(), version, goal, pending: None };
    if node.is_me(&resident.machine) {
        let hold = super::lifecycle::hold(node, &resident.resident).map_err(failure)?;
        view.pending = super::lifecycle::pending(node, &resident.resident, &view, hold.as_ref());
    }
    Ok(view)
}

pub(crate) async fn call(
    node: &Arc<Node>,
    resident: Address,
    account: HubAccount,
    author: Author,
    turn: Option<String>,
    request: Request,
) -> Result<Updated, Failure> {
    if matches!(request, Request::Read) {
        return read(node, &resident, &account).await.map(|view| Updated::Goal { view: Box::new(view) });
    }
    let gate = crate::control_queue::gate(node, &resident.resident);
    let _action = gate.lock().await;
    let place = if node.is_me(&resident.machine) { resident.resident.to_string() } else { resident.key() };
    let _place = node.places.using(&place).await;
    call_held(node, resident, account, author, turn, request).await
}

/// `call` for a caller that already holds the conversation's action gate and
/// keeps it from being deleted meanwhile, such as a handover holding its
/// management gate while it reopens a blocked colleague's goal. It takes
/// neither gate again.
pub(crate) async fn call_held(
    node: &Arc<Node>,
    resident: Address,
    account: HubAccount,
    author: Author,
    turn: Option<String>,
    request: Request,
) -> Result<Updated, Failure> {
    if matches!(request, Request::Read) {
        return read(node, &resident, &account).await.map(|view| Updated::Goal { view: Box::new(view) });
    }
    if node.is_me(&resident.machine) {
        node.residents.active(&resident.resident)?;
    }
    let directory = node.world.owner(&node.names(), &resident);
    if super::scope::pinned_at(&directory)?.is_some_and(|pinned| pinned != account) {
        return Err(Failure::not_run(words::ACCOUNT_CHANGED));
    }
    let operation = match &request {
        Request::Change { operation, .. } | Request::Mark { operation, .. } => operation,
        Request::Read => unreachable!(),
    };
    if operation.is_empty() {
        return Err(Failure::not_run(fastctx_types::goals::words::NO_OPERATION));
    }
    let input_hash = fastctx_hands::files::content_hash(&serde_json::to_vec(&(&request, &author)).map_err(failure)?);
    if let Some(mut intent) = intents::read(&directory, operation).map_err(failure)? {
        if intent.input_hash != input_hash {
            return Err(Failure::not_run(fastctx_types::goals::words::REUSED_OPERATION));
        }
        return finish(node, &mut intent).await;
    }
    let current = read(node, &resident, &account).await?;
    let mut proposed = current.goal.clone();
    let at = Timestamp::now();
    let change = state::Change { operation, author: author.clone(), turn, at };
    match &request {
        Request::Change { expected, change: requested, .. } => {
            if !matches!(author, Author::User { .. }) {
                return Err(Failure::refused(words::OWNER_ONLY));
            }
            if *expected != current.version {
                return Ok(Updated::Conflict { current: current.version });
            }
            match requested {
                UserChange::Clear => proposed = None,
                UserChange::Open { objective } if proposed.is_none() => {
                    let owner = node
                        .residents
                        .about(&resident.resident)
                        .map(|(about, _)| about.origin)
                        .and_then(|origin| match origin {
                            fastctx_types::residents::Origin::Colleague { asked_by, .. } => {
                                Some(Owner::Conversation { address: asked_by })
                            }
                            _ => None,
                        })
                        .unwrap_or(Owner::User);
                    proposed = Some(state::open(resident.clone(), account.clone(), owner, objective.clone(), change)?);
                }
                requested => state::user(
                    proposed.as_mut().ok_or_else(|| Failure::not_run(words::NO_GOAL))?,
                    requested.clone(),
                    change,
                )?,
            }
        }
        Request::Mark { mark, .. } => {
            let Author::Resident { address } = &author else {
                return Err(Failure::refused(words::MAIN_ONLY));
            };
            if !super::same(node, address, &resident) {
                return Err(Failure::refused(words::MAIN_ONLY));
            }
            state::mark(proposed.as_mut().ok_or_else(|| Failure::not_run(words::NO_GOAL))?, mark.clone(), change)?;
        }
        Request::Read => unreachable!(),
    }
    super::scope::keep_at(&directory, &account).map_err(failure)?;
    let mut intent = Intent {
        operation: operation.clone(),
        resident: resident.clone(),
        input_hash,
        account,
        expected: current.version,
        proposed,
        at,
        phase: Phase::Prepared,
    };
    intents::keep(&directory, &intent).map_err(failure)?;
    super::recovery::schedule(node, resident);
    finish(node, &mut intent).await
}

/// The caller holds the conversation's action gate; providers must not reacquire it.
pub(crate) async fn finish(node: &Arc<Node>, intent: &mut Intent) -> Result<Updated, Failure> {
    let directory = node.world.owner(&node.names(), &intent.resident);
    if let Phase::Conflict { current } = intent.phase {
        return Ok(Updated::Conflict { current });
    }
    if !intent.phase.pending() {
        return read(node, &intent.resident, &intent.account).await.map(|view| Updated::Goal { view: Box::new(view) });
    }
    let mut current = read(node, &intent.resident, &intent.account).await?;
    if matches!(intent.phase, Phase::Prepared) {
        let recorded = current.goal.as_ref().is_some_and(|goal| {
            goal.operation == intent.operation
                || goal.decisions.iter().any(|decision| decision.operation == intent.operation)
        });
        let cleared = intent.proposed.is_none()
            && current.goal.is_none()
            && current.version == intent.expected.unwrap_or(0).checked_add(1);
        if recorded || cleared {
            intent.phase = Phase::Accepted { version: current.version.expect("a recognized mutation has a version") };
            intents::keep(&directory, intent).map_err(failure)?;
        } else if current.version != intent.expected {
            intent.phase = Phase::Conflict { current: current.version };
            intent.proposed = None;
            intents::keep(&directory, intent).map_err(failure)?;
            return Ok(Updated::Conflict { current: current.version });
        } else {
            let body = intent.proposed.as_ref().map(serde_json::to_value).transpose().map_err(failure)?;
            match bounded(node.goals.records.exchange(
                &intent.account,
                KIND,
                intent.resident.resident.as_str(),
                intent.expected,
                body,
            ))
            .await?
            {
                Exchanged::Written { version } => {
                    intent.phase = Phase::Accepted { version };
                    intents::keep(&directory, intent).map_err(failure)?;
                    current = read(node, &intent.resident, &intent.account).await?;
                }
                Exchanged::Conflict { current } => {
                    intent.phase = Phase::Conflict { current };
                    intent.proposed = None;
                    intents::keep(&directory, intent).map_err(failure)?;
                    return Ok(Updated::Conflict { current });
                }
            }
        }
    }
    let Phase::Accepted { version } = intent.phase else { unreachable!() };
    if current.version.is_none_or(|current| current < version) {
        return Err(Failure::new(fastctx_types::goals::words::LOST_REVISION));
    }
    if node.is_me(&intent.resident.machine) && !node.residents.is_deleting(&intent.resident.resident) {
        apply(node, &intent.resident.resident, &current).await?;
    }
    intent.phase = if current.version == Some(version) {
        Phase::Applied { version }
    } else {
        Phase::Superseded { current: current.version }
    };
    intent.proposed = None;
    intents::keep(&directory, intent).map_err(failure)?;
    Ok(Updated::Goal { view: Box::new(current) })
}

pub(crate) async fn apply(node: &Arc<Node>, resident: &ResidentId, view: &View) -> Result<(), Failure> {
    let held = super::lifecycle::hold(node, resident).map_err(failure)?;
    let holds = held.as_ref().is_some_and(|held| !held.superseded(view));
    let text = if holds || node.residents.removed(resident).is_some() {
        String::new()
    } else {
        view.goal.as_ref().map(words::context).unwrap_or_default()
    };
    if held.is_some() && !holds {
        super::lifecycle::clear_hold(node, resident).map_err(failure)?;
    }
    bounded(node.goals.context.set(resident, "goal", &text)).await?;
    let mut view = view.clone();
    view.pending = super::lifecycle::pending(node, resident, &view, held.as_ref());
    publish(node, resident, &view);
    Ok(())
}

pub(crate) fn publish(node: &Node, resident: &ResidentId, view: &View) {
    node.goals.views.lock().expect("goal views").insert(resident.clone(), view.clone());
    node.watchers.publish(
        resident,
        &fastctx_types::session::SessionEvent {
            at: Timestamp::now(),
            happened: fastctx_types::session::Happened::Changed {
                turn: None,
                change: fastctx_types::session::SessionChange::Goal {
                    goal: serde_json::json!({"world": true, "view": view}),
                },
            },
        },
    );
    node.goals.changed.notify_one();
}

pub(crate) async fn bounded<T>(future: super::records::Reply<'_, T>) -> Result<T, Failure> {
    tokio::time::timeout(Duration::from_secs(60), future)
        .await
        .map_err(|_| Failure::new(fastctx_types::goals::words::PROVIDER_TIMEOUT))?
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
