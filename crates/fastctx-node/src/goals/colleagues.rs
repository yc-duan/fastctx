//! Colleague birth and the requester's authority over its goal.

use super::{intents, operations, scope, state, words};
use crate::Node;
use fastctx_types::goals::{Author, Goal, Owner, Request, Status, Updated, UserChange, View};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{About, Origin, Pending, PendingState};
use fastctx_types::session::Speaker;
use fastctx_types::{Address, NoticeId, ResidentId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(crate) fn ready(node: &Node) -> Result<(), Failure> {
    if !node.goals.records.available() {
        return Err(Failure::not_run(words::RECORDS_MISSING));
    }
    if !node.goals.context.available() {
        return Err(Failure::not_run(words::CONTEXT_MISSING));
    }
    Ok(())
}

/// Older nodes can accept Ask yet create a colleague without an autonomous goal.
/// A colleague's goal as the collaboration tree draws it.
pub(crate) async fn summary(
    node: &Arc<Node>,
    resident: &ResidentId,
) -> Result<Option<fastctx_types::collab::GoalSummary>, Failure> {
    use fastctx_types::collab::{GoalState, GoalSummary};
    let account = scope::resolve(node, resident, None)?;
    let address = Address { machine: node.machine(), resident: resident.clone() };
    let view = operations::read(node, &address, &account).await?;
    Ok(view.goal.map(|goal| {
        let state = match goal.status {
            Status::Active => GoalState::Active,
            Status::Complete => GoalState::Complete,
            Status::Blocked => GoalState::Blocked,
            Status::Paused => GoalState::Paused,
        };
        let elapsed = (jiff::Timestamp::now().as_millisecond() - goal.created_at.as_millisecond()).max(0) as u64;
        GoalSummary {
            state,
            started_at: goal.created_at,
            elapsed_ms: elapsed,
            id: Some(goal.id),
            parent: goal.parent,
            objective: Some(goal.objective),
        }
    }))
}

/// Open the birth goal once. Retrying creation never replaces or resumes an existing goal.
pub(crate) async fn open(
    node: &Arc<Node>,
    resident: &ResidentId,
    owner: &Address,
    objective: &str,
) -> Result<(), Failure> {
    ready(node)?;
    let (about, _) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
    let Origin::Colleague { asked_by, .. } = about.origin else {
        return Err(Failure::refused(words::OWNER_ONLY));
    };
    if !super::same(node, owner, &asked_by) {
        return Err(Failure::refused(words::OWNER_ONLY));
    }
    let gate = crate::control_queue::gate(node, resident);
    let _action = gate.lock().await;
    let _place = node.places.using(resident.as_str()).await;
    let account = scope::resolve(node, resident, None)?;
    let address = Address { machine: node.machine(), resident: resident.clone() };
    let operation = format!("colleague-open:{resident}");
    if let Some(mut intent) = intents::read(&node.world.conversation(resident), &operation).map_err(failure)? {
        operations::finish(node, &mut intent).await?;
        return Ok(());
    }
    let view = operations::read(node, &address, &account).await?;
    if view.goal.is_some() {
        return Ok(());
    }
    match operations::call_held(
        node,
        address,
        account,
        Author::World,
        None,
        Request::Change {
            operation,
            expected: view.version,
            change: UserChange::Open { objective: objective.to_string(), standing: false },
        },
    )
    .await?
    {
        Updated::Conflict { .. } => Err(Failure::new(words::CLI_CONFLICT)),
        _ => Ok(()),
    }
}

/// Called under the handover action and management gates, before submitting the input.
pub(crate) async fn incoming(
    node: &Arc<Node>,
    resident: &ResidentId,
    about: &About,
    input: &Pending,
) -> Result<(), Failure> {
    if input.state != PendingState::Waiting {
        return Ok(());
    }
    let Origin::Colleague { asked_by, .. } = &about.origin else { return Ok(()) };
    let fastctx_types::residents::Notice::Words { from: Speaker::Resident { address }, .. } = &input.delivery.notice
    else {
        return Ok(());
    };
    if !super::same(node, address, asked_by) || !input.readers.iter().any(|reader| super::same(node, reader, asked_by))
    {
        return Ok(());
    }
    reopen_blocked(node, resident, &input.delivery.id).await
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    delivery: NoticeId,
    attempt: Option<Attempt>,
}

#[derive(Serialize, Deserialize)]
struct Attempt {
    goal: String,
    expected: Option<u64>,
    operation: String,
}

fn attempt(view: &View, delivery: &NoticeId) -> Option<Attempt> {
    let goal = view.goal.as_ref()?;
    (view.pending.is_none() && goal.status == Status::Blocked && goal.retired.is_none()).then(|| Attempt {
        goal: goal.id.clone(),
        expected: view.version,
        operation: format!("colleague-reopen:{delivery}:{}", view.version.unwrap_or(0)),
    })
}

/// A new message resumes only a blocked goal. The caller holds its handover gates.
/// Receipts include no-op decisions, so replay cannot resume a later block or pause.
pub(crate) async fn reopen_blocked(
    node: &Arc<Node>,
    resident: &ResidentId,
    delivery: &NoticeId,
) -> Result<(), Failure> {
    let Some(account) = scope::pinned(node, resident)? else { return Ok(()) };
    let directory = node.world.conversation(resident);
    let path = directory
        .books()
        .join("goal-inputs")
        .join(format!("{}.json", fastctx_hands::files::content_hash(delivery.as_str().as_bytes())));
    let address = Address { machine: node.machine(), resident: resident.clone() };
    let mut receipt: Receipt = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(failure)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let view = operations::read(node, &address, &account).await?;
            let receipt = Receipt { delivery: delivery.clone(), attempt: attempt(&view, delivery) };
            keep(&path, &receipt)?;
            receipt
        }
        Err(error) => return Err(failure(error)),
    };
    if receipt.delivery != *delivery {
        return Err(Failure::new(words::REUSED_OPERATION));
    }
    while let Some(pending) = &receipt.attempt {
        let result = if let Some(mut intent) = intents::read(&directory, &pending.operation).map_err(failure)? {
            operations::finish(node, &mut intent).await?
        } else {
            let view = operations::read(node, &address, &account).await?;
            let Some(goal) = view.goal.as_ref().filter(|goal| goal.id == pending.goal) else {
                receipt.attempt = None;
                keep(&path, &receipt)?;
                return Ok(());
            };
            let Owner::Conversation { address: owner } = &goal.owner else {
                return Err(Failure::refused(words::OWNER_ONLY));
            };
            if view.pending.is_some() {
                receipt.attempt = None;
                keep(&path, &receipt)?;
                return Ok(());
            }
            if view.version != pending.expected {
                Updated::Conflict { current: view.version }
            } else {
                let mut proposed = goal.clone();
                let at = jiff::Timestamp::now();
                // A message may resume a block, but cannot clear a user stop
                // recorded while this account update is in flight.
                state::user(
                    &mut proposed,
                    UserChange::Resume,
                    state::Change {
                        operation: &pending.operation,
                        author: Author::Resident { address: owner.clone() },
                        turn: None,
                        at,
                    },
                )?;
                let mut intent = intents::Intent {
                    operation: pending.operation.clone(),
                    input_hash: pending.operation.clone(),
                    resident: address.clone(),
                    account: account.clone(),
                    expected: pending.expected,
                    proposed: Some(proposed),
                    at,
                    phase: intents::Phase::Prepared,
                };
                intents::keep(&directory, &intent).map_err(failure)?;
                super::recovery::schedule(node, address.clone());
                operations::finish(node, &mut intent).await?
            }
        };
        receipt.attempt = if matches!(result, Updated::Conflict { .. }) {
            let view = operations::read(node, &address, &account).await?;
            attempt(&view, delivery).filter(|next| next.goal == pending.goal)
        } else {
            None
        };
        keep(&path, &receipt)?;
    }
    Ok(())
}

fn keep(path: &std::path::Path, receipt: &Receipt) -> Result<(), Failure> {
    fastctx_platform::ensure_private_dir(path.parent().expect("goal input directory")).map_err(failure)?;
    fastctx_platform::publish(
        path,
        &serde_json::to_vec(receipt).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)
}

pub(super) fn authorize(
    node: &Node,
    resident: &Address,
    goal: Option<&Goal>,
    author: &mut Author,
    change: &UserChange,
) -> Result<(), Failure> {
    match author {
        Author::User { .. } => return Ok(()),
        Author::Resident { address } if matches!(change, UserChange::Pause | UserChange::Resume) => {
            if let Some(Goal { owner: Owner::Conversation { address: owner }, .. }) = goal
                && super::same(node, address, owner)
                && !super::same(node, address, resident)
            {
                *address = owner.clone();
                return Ok(());
            }
        }
        Author::World
            if goal.is_none() && node.is_me(&resident.machine) && matches!(change, UserChange::Open { .. }) =>
        {
            if node
                .residents
                .about(&resident.resident)
                .is_some_and(|(about, _)| matches!(about.origin, Origin::Colleague { .. }))
            {
                return Ok(());
            }
        }
        _ => {}
    }
    Err(Failure::refused(words::OWNER_ONLY))
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
