//! Drives durable goal decisions without holding shared locks across outside work.

mod checkpoint;
mod compaction;
mod driver;
mod effects;
mod judging;
mod material_size;
mod materials;
mod policy;
mod returned;
mod uptime;
mod waiting;
mod words;

use super::{intents, operations, state};
use crate::Node;
use fastctx_types::goals::{Author, Status, Updated, View};
use fastctx_types::ipc::Failure;
use fastctx_types::timers::{Action, Owner};
use fastctx_types::{NoticeId, ResidentId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) use effects::fire;

pub(crate) struct Running {
    epoch: String,
    uptime: uptime::Uptime,
    inputs: Mutex<HashMap<ResidentId, u64>>,
    workers: Mutex<HashMap<ResidentId, bool>>,
    errors: Mutex<HashMap<ResidentId, String>>,
    checkpoints: Mutex<HashMap<ResidentId, checkpoint::Checkpoint>>,
}

impl Default for Running {
    fn default() -> Self {
        Self {
            epoch: crate::speech::new_id("goal-node").to_string(),
            uptime: Default::default(),
            inputs: Default::default(),
            workers: Default::default(),
            errors: Default::default(),
            checkpoints: Default::default(),
        }
    }
}

fn clock(node: &Node) -> policy::Clock {
    let reading = node.goals.engine.uptime.read();
    policy::Clock {
        now: jiff::Timestamp::now(),
        epoch: format!("{}:{}", node.goals.engine.epoch, reading.generation),
        online_ms: reading.online_ms,
    }
}

pub(crate) fn observe_view(node: &Node, id: &ResidentId, view: &View) {
    let mut inputs = node.goals.engine.inputs.lock().expect("goal inputs");
    if view.goal.as_ref().is_some_and(|goal| goal.retired.is_none()) {
        inputs.entry(id.clone()).or_default();
    } else {
        inputs.remove(id);
    }
    drop(inputs);
    if view.goal.as_ref().is_none_or(|goal| !goal.active() || goal.runtime.retry.is_none()) {
        node.goals.engine.checkpoints.lock().expect("goal checkpoints").remove(id);
    }
}

pub(crate) fn requests(node: &Node, resident: &ResidentId, work: &str) -> bool {
    if node.residents.active(resident).is_err() {
        return false;
    }
    node.goals.views.lock().expect("goal views").get(resident).is_some_and(|view| {
        view.pending.is_none()
            && view.goal.as_ref().is_some_and(|goal| {
                goal.retired.is_none()
                    && goal.status != Status::Complete
                    && goal.runtime.pending.as_ref().is_some_and(|pending| pending.id.as_str() == work)
            })
    })
}

/// Called for actual input, a new turn, or completion of initial conversation setup.
/// Retry and status chatter do not count as work becoming ready.
pub(crate) fn input_changed(node: &Node, id: &ResidentId) {
    let changed = {
        let mut inputs = node.goals.engine.inputs.lock().expect("goal inputs");
        if let Some(revision) = inputs.get_mut(id) {
            *revision = revision.wrapping_add(1);
            true
        } else {
            false
        }
    };
    if changed {
        node.goals.changed.send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

fn generation(node: &Node, id: &ResidentId) -> String {
    let revision = node.goals.engine.inputs.lock().expect("goal inputs").get(id).copied().unwrap_or(0);
    let reading = node.goals.engine.uptime.read();
    format!("{}:{}:{revision}", node.goals.engine.epoch, reading.generation)
}

pub(crate) fn timing(node: &Node, id: &ResidentId, view: &View) -> Result<(), Failure> {
    let table = crate::timers::table(node)?;
    let key = format!("goal:{id}");
    let owner = Owner::Goal { resident: id.clone() };
    let current = table.get(&key);
    if current.as_ref().is_some_and(|timer| timer.owner != owner) {
        return Err(Failure::new("the goal's timer ID belongs to another owner"));
    }
    let pending = view
        .goal
        .as_ref()
        .filter(|goal| view.pending.is_none() && goal.retired.is_none() && goal.status != Status::Complete)
        .and_then(|goal| goal.runtime.pending.as_ref().map(|work| (goal, work)));
    if let Some((goal, work)) = pending {
        let action = Action::Goal { resident: id.clone(), goal: goal.id.clone(), work: work.id.clone() };
        if current
            .as_ref()
            .is_none_or(|timer| timer.delivery != work.id || timer.due != work.due || timer.action != action)
        {
            table.set(key, owner, work.due, action, work.id.clone()).map_err(failure)?;
        }
    } else if current.is_some() {
        table.cancel(&key).map_err(failure)?;
    }
    Ok(())
}

pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut changed = node.goals.changed.subscribe();
        let mut residents = node.residents.changes();
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            node.goals.engine.uptime.read();
            let ids: Vec<_> = node
                .goals
                .views
                .lock()
                .expect("goal views")
                .iter()
                .filter(|(_, view)| {
                    view.pending.is_none()
                        && view.goal.as_ref().is_some_and(|goal| {
                            goal.retired.is_none()
                                && (goal.active() || goal.claim.is_some() || goal.runtime.pending.is_some())
                        })
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in ids {
                schedule(&node, id);
            }
            tokio::select! {
                result = changed.changed() => if result.is_err() { return; },
                result = residents.changed() => if result.is_err() { return; },
                _ = tick.tick() => {},
            }
        }
    });
}

fn schedule(node: &Arc<Node>, id: ResidentId) {
    {
        let mut workers = node.goals.engine.workers.lock().expect("goal workers");
        if let Some(dirty) = workers.get_mut(&id) {
            *dirty = true;
            return;
        }
        workers.insert(id.clone(), false);
    }
    let node = node.clone();
    tokio::spawn(async move {
        loop {
            let result = driver::reconcile(&node, &id).await;
            {
                let mut errors = node.goals.engine.errors.lock().expect("goal errors");
                match result {
                    Ok(()) => {
                        errors.remove(&id);
                    }
                    Err(error) => {
                        if errors.get(&id) != Some(&error.message) {
                            node.log.line(format_args!("driving {id}'s goal: {}", error.message));
                            errors.insert(id.clone(), error.message);
                        }
                    }
                }
            }
            let mut workers = node.goals.engine.workers.lock().expect("goal workers");
            if workers.get_mut(&id).is_some_and(std::mem::take) {
                continue;
            }
            workers.remove(&id);
            break;
        }
    });
}

/// The action and place gates belong to the caller. Effects are installed only from accepted records.
async fn commit(node: &Arc<Node>, view: &View, mut decision: policy::Decision) -> Result<(), Failure> {
    let operation = crate::speech::new_id("goal-decision").to_string();
    let at = jiff::Timestamp::now();
    let turn = decision.goal.runtime.settled_turn.clone();
    state::remember(
        &mut decision.goal,
        state::Change { operation: &operation, author: Author::World, turn, at },
        decision.kind,
        decision.reason,
    );
    let resident = decision.goal.resident.clone();
    let directory = node.world.owner(&node.names(), &resident);
    let mut intent = intents::Intent {
        operation: operation.clone(),
        input_hash: operation,
        resident: resident.clone(),
        account: view.account.clone(),
        expected: view.version,
        proposed: Some(decision.goal),
        at,
        phase: intents::Phase::Prepared,
    };
    intents::keep(&directory, &intent).map_err(failure)?;
    super::recovery::schedule(node, resident);
    match operations::finish(node, &mut intent).await? {
        Updated::Goal { .. } => Ok(()),
        Updated::History { .. } => Err(Failure::new("a goal mutation returned history")),
        Updated::Conflict { .. } => {
            node.goals.changed.send_modify(|revision| *revision = revision.wrapping_add(1));
            Ok(())
        }
    }
}

fn new_work() -> NoticeId {
    crate::speech::new_id("goal-work")
}
fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
