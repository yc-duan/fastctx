//! A local user stop precedes the host abort, so Claude cannot flush queued work while its account is offline.

use super::{intents, lifecycle, operations, state};
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::goals::{Author, Status, Stop, StopKind, View};
use fastctx_types::ipc::Failure;
use std::sync::Arc;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BridgeState {
    active: bool,
    paused: bool,
    held: Option<Vec<fastctx_types::NoticeId>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Local {
    #[serde(flatten)]
    view: View,
    #[serde(default, rename = "_claude_goal", skip_serializing_if = "Option::is_none")]
    bridge: Option<BridgeState>,
}

/// The caller holds the resident-storage transaction, which also orders new input.
pub(crate) fn publish(node: &Node, id: &ResidentId, view: &View) -> Result<(), Failure> {
    publish_mode(node, id, view, false)
}

fn publish_mode(node: &Node, id: &ResidentId, view: &View, refreeze: bool) -> Result<(), Failure> {
    use fastctx_hosts::claude::control as files;
    let path = node.world.conversation(id).books().join("goal-local.json");
    let about = node.residents.about(id).map(|(about, _)| about).filter(|about| about.host.agent.as_str() == "claude");
    let exchange = about
        .as_ref()
        .and_then(|about| about.session.as_ref())
        .map(|session| fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session));
    let bridge = if about.is_some() {
        let prior = files::read::<Local>(&path).map_err(failure)?.and_then(|local| local.bridge);
        let prior = match (prior, exchange.as_ref()) {
            (None, Some(exchange)) => files::read::<BridgeState>(&exchange.join("goal.json")).map_err(failure)?,
            (prior, _) => prior,
        };
        Some(bridge_state(node, id, view, prior, refreeze)?)
    } else {
        None
    };
    // Frozen input IDs and the accepted local view must survive together,
    // including the interval before a Claude session has a directory.
    let local = Local { view: view.clone(), bridge };
    files::write(&path, &local).map_err(failure)?;
    if let (Some(exchange), Some(bridge)) = (exchange, &local.bridge)
        && exchange.is_dir()
    {
        let exchange = std::fs::canonicalize(exchange).map_err(failure)?;
        files::write(&exchange.join("goal.json"), bridge).map_err(failure)?;
    }
    Ok(())
}

/// Replays the latest local state into a newly identified Claude activation.
/// It runs before that activation's initial policy is marked ready.
pub(crate) fn registered(node: &Node, id: &ResidentId, exchange: &std::path::Path) -> Result<(), Failure> {
    use fastctx_hosts::claude::control as files;
    let _transaction = crate::resident_storage::lock();
    let path = node.world.conversation(id).books().join("goal-local.json");
    let Some(mut local) = files::read::<Local>(&path).map_err(failure)? else { return Ok(()) };
    if local.bridge.is_none() {
        let prior = files::read::<BridgeState>(&exchange.join("goal.json")).map_err(failure)?;
        local.bridge = Some(bridge_state(node, id, &local.view, prior, false)?);
        files::write(&path, &local).map_err(failure)?;
    }
    let exchange = std::fs::canonicalize(exchange).map_err(failure)?;
    files::write(&exchange.join("goal.json"), local.bridge.as_ref().expect("known Claude goal state")).map_err(failure)
}

fn bridge_state(
    node: &Node,
    id: &ResidentId,
    view: &View,
    previous: Option<BridgeState>,
    refreeze: bool,
) -> Result<BridgeState, Failure> {
    let paused = view.pending.is_some()
        || view.goal.as_ref().is_some_and(|goal| goal.status == Status::Paused && goal.retired.is_none());
    let held = if !paused {
        Vec::new()
    } else if !refreeze && previous.as_ref().is_some_and(|state| state.paused && state.held.is_some()) {
        previous.and_then(|state| state.held).expect("known held inputs")
    } else {
        crate::inbox::pending_here(&node.world.conversation(id))
            .map_err(failure)?
            .into_iter()
            .map(|pending| pending.delivery.id)
            .collect()
    };
    Ok(BridgeState { active: view.active(), paused, held: Some(held) })
}

pub(crate) fn stop(node: &Arc<Node>, id: &ResidentId, turn: Option<String>) -> Result<(), Failure> {
    let _transaction = crate::resident_storage::lock();
    let directory = node.world.conversation(id);
    let cached = node.goals.views.lock().expect("goal views").get(id).cloned();
    let mut view = match cached {
        Some(view) => view,
        None => match std::fs::read(directory.books().join("goal-local.json")) {
            Ok(bytes) => serde_json::from_slice::<View>(&bytes).map_err(failure)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(failure(error)),
        },
    };
    let Some(mut goal) = view.goal.clone().filter(|goal| goal.retired.is_none() && goal.status != Status::Complete)
    else {
        return Ok(());
    };
    let operation = crate::speech::new_id("goal-stop").to_string();
    let at = jiff::Timestamp::now();
    let reason = "The user stopped this turn; the goal is paused.".to_string();
    goal.status = Status::Paused;
    goal.stop = Some(Stop { kind: StopKind::User, reason: reason.clone(), resume_at: None });
    goal.claim = None;
    goal.runtime.pending = None;
    goal.runtime.retry = None;
    state::remember(
        &mut goal,
        state::Change { operation: &operation, author: Author::World, turn, at },
        "user_stopped",
        reason.clone(),
    );
    let intent = intents::Intent {
        operation: operation.clone(),
        input_hash: operation.clone(),
        resident: goal.resident.clone(),
        account: view.account.clone(),
        expected: view.version,
        proposed: Some(goal.clone()),
        at,
        phase: intents::Phase::Prepared,
    };
    intents::keep(&directory, &intent).map_err(failure)?;
    let held = lifecycle::Hold {
        base: view.version,
        kind: lifecycle::HoldKind::UserStop,
        at,
        goal: goal.id,
        operation,
        reason,
    };
    fastctx_platform::publish(
        &directory.books().join("goal-hold.json"),
        &serde_json::to_vec(&held).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)?;
    view.pending = Some(held.pending());
    publish_mode(node, id, &view, true)?;
    operations::publish(node, id, &view);
    super::recovery::schedule(node, intent.resident);
    Ok(())
}
