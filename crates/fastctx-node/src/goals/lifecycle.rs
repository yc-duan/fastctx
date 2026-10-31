//! Local lifecycle facts end autonomous work immediately; account changes remain recoverable.

use super::{
    intents::{self, Intent, Phase},
    operations, scope, state, words,
};
use crate::Node;
use fastctx_types::authorization::DenialReason;
use fastctx_types::goals::{Author, Retired, Retirement, Status, Stop, StopKind, View};
use fastctx_types::ipc::Failure;
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;
use std::sync::Arc;
use std::time::Duration;

pub(crate) use super::hold::{Hold, HoldKind};

pub(crate) fn pending(
    node: &Node,
    id: &ResidentId,
    view: &View,
    held: Option<&Hold>,
) -> Option<fastctx_types::goals::Pending> {
    if let Some(held) = held
        && !held.superseded(view)
    {
        return Some(held.pending());
    }
    if let Some(at) = node.residents.removed(id)
        && view.goal.as_ref().is_some_and(|goal| goal.retired.is_none() && goal.status != Status::Complete)
    {
        return Some(fastctx_types::goals::Pending {
            action: fastctx_types::goals::PendingAction::End,
            reason: words::ARCHIVED.into(),
            at,
        });
    }
    None
}

/// Whether this conversation's goal still has a step to finish here: an
/// operation not yet applied, or a hold not yet settled.
pub(crate) fn unfinished(directory: &fastctx_types::layout::ConversationDir) -> std::io::Result<bool> {
    super::local::prepare_hold(directory)?;
    Ok(crate::records::read(directory, |rows| Ok(super::local::hold(rows)?.is_some()))?
        || !super::intents::pending(directory)?.is_empty())
}

pub(crate) async fn hold(node: &Arc<Node>, id: &ResidentId) -> std::io::Result<Option<Hold>> {
    let directory = node.world.conversation(id);
    tokio::task::spawn_blocking(move || {
        super::local::prepare_hold(&directory)?;
        crate::records::read(&directory, super::local::hold)
    })
    .await
    .map_err(std::io::Error::other)?
}

pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut residents = node.residents.changes();
        let mut views = node.views.subscribe();
        let mut goals = node.goals.changed.subscribe();
        let mut every = tokio::time::interval(Duration::from_secs(30));
        loop {
            let candidates: Vec<_> = node
                .goals
                .views
                .lock()
                .expect("goal views")
                .iter()
                .filter(|(_, view)| view.goal.as_ref().is_some_and(|goal| goal.retired.is_none()))
                .map(|(id, _)| id.clone())
                .collect();
            for id in candidates {
                if !node.goals.lifecycle.lock().expect("goal lifecycle").insert(id.clone()) {
                    continue;
                }
                let node = node.clone();
                tokio::spawn(async move {
                    if let Err(error) = reconcile(&node, &id).await {
                        node.log.line(format_args!("reconciling {id}'s goal lifecycle: {}", error.message));
                    }
                    node.goals.lifecycle.lock().expect("goal lifecycle").remove(&id);
                });
            }
            tokio::select! {
                result = residents.changed() => { if result.is_err() { return; } },
                result = views.changed() => { if result.is_err() { return; } },
                result = goals.changed() => { if result.is_err() { return; } },
                _ = every.tick() => {},
            }
        }
    });
}

async fn reconcile(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let gate = crate::control_queue::gate(node, id);
    let _action = gate.lock().await;
    let _place = node.places.using(id.as_str()).await;
    if node.residents.is_deleting(id) {
        return Ok(());
    }
    let Some(view) = node.goals.views.lock().expect("goal views").get(id).cloned() else { return Ok(()) };
    let mut held = hold(node, id).await.map_err(failure)?;
    let view = if let Some(held) = &held {
        let directory = node.world.conversation(id);
        if intents::read(&directory, &held.operation)
            .await
            .map_err(failure)?
            .is_some_and(|intent| intent.phase.pending())
        {
            super::recovery::schedule(node, Address { machine: node.machine(), resident: id.clone() });
            return Ok(());
        }
        operations::read(node, &Address { machine: node.machine(), resident: id.clone() }, &view.account).await?
    } else {
        view
    };
    let (directory, checking) = (node.world.conversation(id), view.clone());
    held = tokio::task::spawn_blocking(move || {
        crate::records::write_at(&directory, fastctx_storage::Priority::Interactive, 1024, move |rows| {
            let held = super::local::hold(rows)?;
            if held.as_ref().is_some_and(|held| held.superseded(&checking)) {
                super::local::clear_hold(rows)?;
                Ok(None)
            } else {
                Ok(held)
            }
        })
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    let Some(mut goal) = view.goal.clone() else { return Ok(()) };
    if goal.retired.is_some() {
        return Ok(());
    }
    let (kind, reason) = if let Some(held) = &held {
        (
            match held.kind {
                HoldKind::Archived => "archived",
                HoldKind::Assistance => "assistance_ended",
                HoldKind::UserStop => "user_stopped",
            },
            held.reason.clone(),
        )
    } else if node.residents.removed(id).is_some() {
        ("archived", words::ARCHIVED.to_string())
    } else {
        if !goal.active() {
            return Ok(());
        }
        let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
        if !crate::assists::assisting(node, &about.acting) {
            return Ok(());
        }
        match crate::assists::permitted(node, &about, &view.account) {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.denial,
                    Some(
                        DenialReason::LoginNotAssisting
                            | DenialReason::MachineNotShared
                            | DenialReason::AccountMissing
                            | DenialReason::AccountLocked
                            | DenialReason::CannotAct
                    )
                ) =>
            {
                ("assistance_ended", error.message)
            }
            Err(_) => return Ok(()),
        }
    };
    let operation = format!("lifecycle:{}:{}:{kind}", goal.id, view.version.unwrap_or(0));
    let at = held.as_ref().map_or_else(Timestamp::now, |held| held.at);
    let base = held.as_ref().map_or(view.version, |held| held.base);
    let hold_kind = match kind {
        "archived" => HoldKind::Archived,
        "user_stopped" => HoldKind::UserStop,
        _ => HoldKind::Assistance,
    };
    let hold_operation = held
        .as_ref()
        .filter(|held| matches!(held.kind, HoldKind::UserStop))
        .map_or_else(|| operation.clone(), |held| held.operation.clone());
    if kind == "user_stopped" {
        goal.runtime.clears_stop = Some(hold_operation.clone());
    }
    goal.runtime.pending = None;
    if kind == "archived" {
        goal.retired = Some(Retired { at, why: Retirement::Archived });
    } else {
        goal.status = Status::Paused;
        goal.stop = Some(Stop {
            kind: if kind == "user_stopped" { StopKind::User } else { StopKind::Assistance },
            reason: reason.clone(),
            resume_at: None,
        });
        goal.claim = None;
    }
    state::remember(
        &mut goal,
        state::Change { operation: &operation, author: Author::World, turn: None, at },
        kind,
        reason.clone(),
    );
    let directory = node.world.conversation(id);
    let intent = Intent {
        access: None,
        operation: operation.clone(),
        resident: goal.resident.clone(),
        account: view.account.clone(),
        input_hash: operation.clone(),
        expected: view.version,
        proposed: Some(goal.clone()),
        at,
        phase: Phase::Prepared,
    };
    let expected = held.as_ref().map(|hold| hold.operation.clone());
    let next = Hold { goal: goal.id, operation: hold_operation, reason, base, at, kind: hold_kind };
    let saving = intent.clone();
    let committed = intents::committed(node, &intent);
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        super::local::prepare_hold(&directory)?;
        intents::prepare(&directory)?;
        let bytes = serde_json::to_vec(&saving)?.len() + serde_json::to_vec(&next)?.len();
        crate::records::write_then_at(
            &directory,
            fastctx_storage::Priority::Interactive,
            bytes,
            move |rows| {
                intents::keep_in(rows, &saving)?;
                if super::local::hold(rows)?.as_ref().map(|hold| &hold.operation) == expected.as_ref() {
                    super::local::put_hold(rows, &next)?;
                }
                Ok(())
            },
            committed,
        )
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    super::recovery::schedule(node, intent.resident.clone());
    operations::apply(node, id, &view).await?;
    let mut intent = intent;
    operations::finish(node, &mut intent).await.map(|_| ())
}

/// Removal retains the goal's account identity until deletion has collected its account records.
pub(crate) fn keeps_books(node: &Node, id: &ResidentId) -> Result<bool, Failure> {
    Ok(scope::pinned(node, id)?.is_some())
}

/// Called inside the durable deletion transaction, with its place and management gates already held.
pub(crate) async fn delete(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let Some(account) = scope::pinned(node, id)? else { return Ok(()) };
    let resident = Address { machine: node.machine(), resident: id.clone() };
    super::past::forget(node, &account, &resident).await?;
    let current = operations::read(node, &resident, &account).await?;
    if current.goal.is_none() && current.version.is_some() {
        super::archive::for_resident(node, &account, id).await?;
        operations::publish(node, id, &current);
        node.goals.views.lock().expect("goal views").remove(id);
        return Ok(());
    }
    let operation = format!("goal-delete:{id}:{}", current.version.unwrap_or(0));
    let directory = node.world.conversation(id);
    let mut intent = match intents::read(&directory, &operation).await.map_err(failure)? {
        Some(intent) => intent,
        None => {
            let intent = Intent {
                access: None,
                operation: operation.clone(),
                input_hash: operation,
                resident,
                account,
                expected: current.version,
                proposed: None,
                at: Timestamp::now(),
                phase: Phase::Prepared,
            };
            intents::keep(node, &directory, &intent).await.map_err(failure)?;
            intent
        }
    };
    match operations::finish(node, &mut intent).await? {
        fastctx_types::goals::Updated::Goal { view } if view.goal.is_none() => {
            operations::publish(node, id, &view);
            node.goals.views.lock().expect("goal views").remove(id);
            Ok(())
        }
        _ => Err(Failure::new(words::DELETE_CHANGED)),
    }
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
