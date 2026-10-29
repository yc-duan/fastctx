//! Serialize a local decision, compare it at the account, then apply the authoritative context.

use super::{
    intents::{self, Intent, Phase},
    records::Exchanged,
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

pub(crate) async fn read(node: &Arc<Node>, resident: &Address, account: &HubAccount) -> Result<View, Failure> {
    loaded(node, resident, account).await.map(|(view, _)| view)
}

async fn loaded(
    node: &Arc<Node>,
    resident: &Address,
    account: &HubAccount,
) -> Result<(View, super::archive::Storage), Failure> {
    let (mut view, storage) = by_id(node, &resident.resident, account).await?;
    if view.goal.as_ref().is_some_and(|goal| !super::same(node, &goal.resident, resident)) {
        return Err(Failure::new(words::CONVERSATION_CHANGED));
    }
    if node.is_me(&resident.machine) {
        let hold = super::lifecycle::hold(node, &resident.resident).map_err(failure)?;
        view.pending = super::lifecycle::pending(node, &resident.resident, &view, hold.as_ref());
    }
    Ok((view, storage))
}

/// Looks up a goal in an already authorized account, including its authoritative home.
pub(crate) async fn by_id(
    node: &Arc<Node>,
    resident: &ResidentId,
    account: &HubAccount,
) -> Result<(View, super::archive::Storage), Failure> {
    let record = bounded(node.goals.records.read(account, KIND, resident.as_str())).await?;
    let version = record.as_ref().map(|record| record.version);
    let (goal, storage): (Option<Goal>, _) = match record.and_then(|record| record.body) {
        Some(body) => {
            let (body, storage) = super::archive::decode(node, account, body).await?;
            (Some(serde_json::from_value(body).map_err(failure)?), storage)
        }
        None => (None, Default::default()),
    };
    if let Some(goal) = &goal {
        if goal.account != *account {
            return Err(Failure::new(words::ACCOUNT_CHANGED));
        }
        if goal.resident.resident != *resident {
            return Err(Failure::new(words::CONVERSATION_CHANGED));
        }
    }
    Ok((View { account: account.clone(), version, goal, pending: None }, storage))
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
    if matches!(request, Request::Past) {
        // The current goal is never among the earlier ones, even when kept
        // there just before a change that did not land.
        let view = read(node, &resident, &account).await?;
        let mut past = super::past::read(node, &resident, &account).await?;
        if let Some(goal) = &view.goal {
            past.goals.retain(|kept| kept.id != goal.id);
        }
        return Ok(Updated::Past { past });
    }
    if let Request::History { page } = &request {
        let view = read(node, &resident, &account).await?;
        let history = match view.goal {
            Some(goal) => super::archive::history(node, &goal, page.as_deref()).await?,
            None if page.is_none() => None,
            None => return Err(Failure::new("the goal whose history was requested no longer exists")),
        };
        return Ok(Updated::History { page: history.map(Box::new) });
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
    mut author: Author,
    turn: Option<String>,
    request: Request,
) -> Result<Updated, Failure> {
    if matches!(request, Request::Read) {
        return read(node, &resident, &account).await.map(|view| Updated::Goal { view: Box::new(view) });
    }
    if node.is_me(&resident.machine) {
        node.residents.active(&resident.resident)?;
        // A goal is worked at by turns the World starts: only a conversation
        // it holds has one (`world-layers.md`).
        if matches!(&request, Request::Change { change: UserChange::Open { .. } | UserChange::Resume, .. })
            && node.residents.about(&resident.resident).is_some_and(|(about, _)| !about.world_managed())
        {
            return Err(Failure::refused(words::NATIVE_CODEX));
        }
    }
    let directory = node.world.owner(&node.names(), &resident);
    if super::scope::pinned_at(&directory)?.is_some_and(|pinned| pinned != account) {
        return Err(Failure::not_run(words::ACCOUNT_CHANGED));
    }
    let operation = match &request {
        Request::Change { operation, .. } | Request::Mark { operation, .. } => operation,
        Request::Read | Request::History { .. } | Request::Past => unreachable!(),
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
    if let Request::Change { change, .. } = &request {
        super::colleagues::authorize(node, &resident, current.goal.as_ref(), &mut author, change)?;
    }
    let change = state::Change { operation, author: author.clone(), turn, at };
    match &request {
        Request::Change { expected, change: requested, .. } => {
            if *expected != current.version {
                return Ok(Updated::Conflict { current: current.version });
            }
            // A goal done, or cleared, joins the earlier goals; a new one set
            // after it is a goal of its own (`world-goal.md`, 目标的历史).
            let replaced = match (requested, current.goal.as_ref()) {
                (UserChange::Clear, Some(goal)) => Some(goal.clone()),
                (UserChange::Open { .. }, Some(goal)) if goal.status == fastctx_types::goals::Status::Complete => {
                    Some(goal.clone())
                }
                _ => None,
            };
            if let Some(goal) = &replaced {
                super::past::keep(node, goal, at).await?;
                if matches!(requested, UserChange::Open { .. }) {
                    proposed = None;
                }
            }
            match requested {
                UserChange::Clear => proposed = None,
                UserChange::Open { objective, standing } if proposed.is_none() => {
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
                    // The goal it serves. A read that failed is not taken for
                    // no goal: the opening fails and is asked again, since a
                    // goal opened without its parent would stay without it.
                    let parent = match &owner {
                        Owner::Conversation { address } => {
                            read(node, address, &account).await?.goal.map(|goal| goal.id)
                        }
                        Owner::User => None,
                    };
                    proposed = Some(state::open(
                        resident.clone(),
                        account.clone(),
                        owner,
                        objective.clone(),
                        *standing,
                        parent,
                        change,
                    )?);
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
        Request::Read | Request::History { .. } | Request::Past => unreachable!(),
    }
    if node.is_me(&resident.machine)
        && matches!(&request, Request::Change { change: UserChange::Open { .. } | UserChange::Resume, .. })
        && let Some(goal) = proposed.as_mut()
    {
        goal.runtime.clears_stop = super::lifecycle::hold(node, &resident.resident)
            .map_err(failure)?
            .filter(|held| matches!(held.kind, super::lifecycle::HoldKind::UserStop))
            .map(|held| held.operation);
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
    let (mut current, storage) = loaded(node, &intent.resident, &intent.account).await?;
    if matches!(intent.phase, Phase::Prepared) {
        let recorded = if let Some(goal) = &current.goal {
            goal.operation == intent.operation
                || goal.decisions.iter().any(|decision| decision.operation == intent.operation)
                || (current.version != intent.expected
                    && super::archive::contains(node, goal, &intent.operation).await?)
        } else {
            false
        };
        let cleared = intent.proposed.is_none()
            && current.goal.is_none()
            && current.version == intent.expected.unwrap_or(0).checked_add(1);
        if recorded || cleared {
            let version =
                intent.expected.unwrap_or(0).checked_add(1).ok_or_else(|| Failure::new("goal revision overflow"))?;
            intent.phase = Phase::Accepted { version };
            intents::keep(&directory, intent).map_err(failure)?;
        } else if current.version != intent.expected {
            super::archive::close(node, &intent.account, &intent.resident.resident, &intent.operation).await?;
            super::archive::for_resident(node, &intent.account, &intent.resident.resident).await?;
            intent.phase = Phase::Conflict { current: current.version };
            intent.proposed = None;
            intents::keep(&directory, intent).map_err(failure)?;
            return Ok(Updated::Conflict { current: current.version });
        } else {
            let body = match &intent.proposed {
                Some(goal) => Some(
                    super::archive::encode(node, &intent.account, goal, &intent.operation, intent.expected, storage)
                        .await?,
                ),
                None => None,
            };
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
                    super::archive::close(node, &intent.account, &intent.resident.resident, &intent.operation).await?;
                    super::archive::for_resident(node, &intent.account, &intent.resident.resident).await?;
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
    if node.is_me(&intent.resident.machine) {
        super::replies::remember(node, &intent.resident.resident, &current).map_err(failure)?;
    }
    if let Some(goal) = &intent.proposed {
        super::reports::keep(node, goal)?;
    }
    super::archive::close(node, &intent.account, &intent.resident.resident, &intent.operation).await?;
    if intent.proposed.is_none() && current.goal.is_none() {
        super::archive::for_resident(node, &intent.account, &intent.resident.resident).await?;
    }
    if node.is_me(&intent.resident.machine) && !node.residents.is_deleting(&intent.resident.resident) {
        apply(node, &intent.resident.resident, &current).await?;
    }
    // A goal cleared, completed or retired with its conversation (archived,
    // removed) takes its sentinel with it; ending one already gone does
    // nothing, so a replay is harmless.
    if node.is_me(&intent.resident.machine)
        && current
            .goal
            .as_ref()
            .is_none_or(|goal| goal.status == fastctx_types::goals::Status::Complete || goal.retired.is_some())
    {
        crate::sentinels::goal_seams::retire(node, intent.account.clone(), intent.resident.clone());
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
    super::replies::remember(node, resident, view).map_err(failure)?;
    if let Some(goal) = &view.goal {
        super::reports::keep(node, goal)?;
    }
    let mut installed = None;
    loop {
        let (text, accepted) = {
            let _transaction = crate::resident_storage::lock();
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
            let mut view = view.clone();
            view.pending = super::lifecycle::pending(node, resident, &view, held.as_ref());
            if installed.as_ref() == Some(&text) {
                super::holding::publish(node, resident, &view)?;
                publish(node, resident, &view);
                (text, Some(view))
            } else {
                if view.pending.is_some()
                    || view.goal.as_ref().is_some_and(|goal| goal.status == fastctx_types::goals::Status::Paused)
                {
                    super::holding::publish(node, resident, &view)?;
                }
                (text, None)
            }
        };
        if let Some(accepted) = accepted {
            return super::engine::timing(node, resident, &accepted);
        }
        bounded(node.goals.context.set(resident, "goal", &text, words::changed(view.goal.as_ref()))).await?;
        installed = Some(text);
    }
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
    super::engine::observe_view(node, resident, view);
    node.goals.changed.send_modify(|revision| *revision = revision.wrapping_add(1));
}

pub(crate) async fn bounded<T>(future: super::records::Reply<'_, T>) -> Result<T, Failure> {
    tokio::time::timeout(Duration::from_secs(60), future)
        .await
        .map_err(|_| Failure::new(fastctx_types::goals::words::PROVIDER_TIMEOUT))?
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
