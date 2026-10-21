//! Durable between-turn controls and a shared admission order for words and controls.

use crate::Node;
use fastctx_hosts::claude::control as files;
use fastctx_platform::doing::{Mutex, Rank};
use fastctx_types::session::control::{Control, ControlState, Controlled, PendingControl};
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use fastctx_types::{ResidentId, ipc::Failure};
use std::sync::Arc;

pub(crate) type Actions = crate::leases::Registry<ResidentId, Mutex<()>>;

pub(crate) fn gate(node: &Node, id: &ResidentId) -> crate::leases::Lease<ResidentId, Mutex<()>> {
    node.actions.get_or_insert_with(id, || Mutex::new(Rank::ACTION, "conversation actions", id.to_string(), ()))
}

fn path(node: &Node, id: &ResidentId) -> std::path::PathBuf {
    node.world.conversation(id).path().join("control-queue.json")
}

pub(crate) fn pending(node: &Node, id: &ResidentId) -> Result<Vec<PendingControl>, Failure> {
    let _transaction = crate::resident_storage::lock();
    read(node, id)
}

fn read(node: &Node, id: &ResidentId) -> Result<Vec<PendingControl>, Failure> {
    files::read(&path(node, id)).map(|rows| rows.unwrap_or_default()).map_err(failure)
}

fn save(node: &Node, id: &ResidentId, rows: &[PendingControl]) -> Result<(), Failure> {
    files::write(&path(node, id), &rows).map_err(failure)
}

fn event(node: &Node, id: &ResidentId, row: &PendingControl) -> Result<(), Failure> {
    let event = SessionEvent {
        at: jiff::Timestamp::now(),
        happened: Happened::Changed {
            turn: None,
            change: SessionChange::Metadata { metadata: serde_json::json!({"fastctx_control": row}) },
        },
    };
    let key = format!("{}:{:?}", row.id, row.state);
    if crate::delivery::event(&node.world.conversation(id), &key, &event).map_err(failure)? {
        node.watchers.publish(id, &event);
    }
    Ok(())
}

pub(crate) fn between_turns(control: &Control) -> bool {
    matches!(control, Control::Compact | Control::Rewind { .. } | Control::Fork { .. })
        || matches!(control, Control::Command { name, path: None, .. }
            if matches!(name.as_str(), "compact" | "review" | "code-review" | "fork"))
}

fn busy(node: &Node, id: &ResidentId) -> Result<bool, Failure> {
    if node.residents.now(id).is_some_and(|now| now.turn.is_some()) {
        return Ok(true);
    }
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new("unknown conversation"))?;
    if about.host.agent.as_str() == "codex"
        && let Some(thread) = about.session.as_deref()
    {
        return Ok(node.codex.of(node, &about)?.running_turn(thread).is_some());
    }
    Ok(false)
}

/// Called with the action gate held. No gate is held while waiting for a turn to end.
pub(crate) fn enqueue_if_busy(
    node: &Arc<Node>,
    id: &ResidentId,
    control: &Control,
) -> Result<Option<Controlled>, Failure> {
    if !between_turns(control) || !busy(node, id)? {
        return Ok(None);
    }
    let row = PendingControl {
        id: crate::speech::new_id("control").to_string(),
        control: control.clone(),
        at: jiff::Timestamp::now(),
        state: ControlState::Waiting,
        result: None,
        error: None,
    };
    {
        let _transaction = crate::resident_storage::lock();
        let mut rows = read(node, id)?;
        rows.push(row.clone());
        save(node, id, &rows)?;
    }
    event(node, id, &row)?;
    schedule(node, id);
    Ok(Some(Controlled::Reported { said: Some("Waiting for the current turn to end.".into()), queued: Some(row.id) }))
}

/// Cancellation competes with the durable claim, not the long-running host operation.
pub(crate) fn cancel(node: &Arc<Node>, id: &ResidentId, key: &str) -> Result<Controlled, Failure> {
    node.residents.active(id)?;
    let row = {
        let _transaction = crate::resident_storage::lock();
        let mut rows = read(node, id)?;
        let row =
            rows.iter_mut().find(|row| row.id == key).ok_or_else(|| Failure::new("no waiting control with this id"))?;
        if row.state != ControlState::Waiting {
            return Err(Failure::new("this control has already entered the host and cannot be withdrawn"));
        }
        row.state = ControlState::Cancelled;
        let row = row.clone();
        save(node, id, &rows)?;
        row
    };
    finish(node, id, &row)?;
    schedule(node, id);
    Ok(Controlled::Done { said: Some("The waiting control was withdrawn.".into()) })
}

fn finish(node: &Node, id: &ResidentId, row: &PendingControl) -> Result<(), Failure> {
    event(node, id, row)?;
    let _transaction = crate::resident_storage::lock();
    let mut rows = read(node, id)?;
    rows.retain(|entry| entry.id != row.id);
    save(node, id, &rows)
}

/// Runs older waiting controls before a newly admitted action, while holding its action gate.
pub(crate) fn drain<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Failure>> + Send + 'a>> {
    Box::pin(async move {
        loop {
            node.residents.active(id)?;
            if busy(node, id)? {
                return Ok(());
            }
            if let Some(first) = pending(node, id)?.first() {
                let directory = node.world.conversation(id);
                for words in crate::inbox::pending(&directory).map_err(failure)? {
                    if words.delivery.notice.along().is_none()
                        && words.delivery.at < first.at
                        && crate::delivery::input_turn(&directory, &words.delivery.id).map_err(failure)?.is_none()
                        && crate::delivery::input_uncertainty(&directory, &words.delivery.id)
                            .map_err(failure)?
                            .is_none()
                    {
                        return Ok(());
                    }
                }
            }
            let row = {
                let _transaction = crate::resident_storage::lock();
                let mut rows = read(node, id)?;
                let Some(row) = rows.first_mut() else {
                    return Ok(());
                };
                if row.state != ControlState::Waiting {
                    let mut row = row.clone();
                    if row.state == ControlState::Running {
                        row.state = ControlState::Uncertain;
                        row.error = Some("the previous control attempt did not retain its result; its outcome is unknown and it was not repeated".into());
                    }
                    drop(_transaction);
                    finish(node, id, &row)?;
                    continue;
                }
                row.state = ControlState::Running;
                let row = row.clone();
                save(node, id, &rows)?;
                row
            };
            event(node, id, &row)?;
            let result = Box::pin(crate::control::execute(node, id, row.control.clone())).await;
            let mut row = row;
            match result {
                Ok(result) => {
                    row.state = ControlState::Completed;
                    row.result = Some(Box::new(result));
                }
                Err(error) => {
                    row.state = ControlState::Failed;
                    row.error = Some(error.message);
                }
            }
            {
                let _transaction = crate::resident_storage::lock();
                let mut rows = read(node, id)?;
                if let Some(saved) = rows.iter_mut().find(|saved| saved.id == row.id) {
                    *saved = row.clone();
                }
                save(node, id, &rows)?;
            }
            finish(node, id, &row)?;
        }
    })
}

pub(crate) fn schedule(node: &Arc<Node>, id: &ResidentId) {
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move {
        node.ready().await;
        let gate = gate(&node, &id);
        let _action = gate.lock().await;
        if node.residents.active(&id).is_ok()
            && let Err(error) = drain(&node, &id).await
        {
            node.log.line(format_args!("executing {id}'s queued controls: {error}"));
        }
    });
}

pub(crate) fn recover(node: &Arc<Node>) {
    for resident in node.residents.everyone().filter(|resident| resident.removed.is_none()) {
        let id = &resident.id;
        let recovered = (|| {
            for mut row in pending(node, id)? {
                if row.state == ControlState::Running {
                    row.state = ControlState::Uncertain;
                    row.error = Some("the node stopped while this control was entering the host; its outcome is unknown and it was not repeated".into());
                    finish(node, id, &row)?;
                } else if row.state != ControlState::Waiting {
                    finish(node, id, &row)?;
                }
            }
            Ok::<_, Failure>(())
        })();
        if let Err(error) = recovered {
            node.log.line(format_args!("recovering {id}'s controls: {error}"));
        }
        schedule(node, id);
    }
}

fn failure(error: std::io::Error) -> Failure {
    Failure::new(format!("recording conversation controls: {error}"))
}
