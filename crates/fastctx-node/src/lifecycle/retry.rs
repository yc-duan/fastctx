//! Durable progress before a host accepts work. Waiting never owns a gate.

use crate::Node;
use fastctx_types::{
    NoticeId, ResidentId,
    ipc::Failure,
    residents::{Landed, PendingState},
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
pub(crate) struct Retrying(Mutex<HashMap<ResidentId, Arc<tokio::sync::Notify>>>);

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Progress {
    pub began: Timestamp,
    pub next: Timestamp,
    failures: u32,
    pub reason: Option<String>,
    pub attempted: bool,
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub after_activation: Option<String>,
}

fn path(node: &Node, id: &ResidentId, input: &NoticeId) -> PathBuf {
    node.world.conversation(id).book("host-progress").join(format!("{input}.json"))
}

pub(crate) fn progress(node: &Node, id: &ResidentId, input: &NoticeId) -> io::Result<Option<Progress>> {
    match std::fs::read(path(node, id, input)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn keep(node: &Node, id: &ResidentId, input: &NoticeId, progress: &Progress) -> io::Result<()> {
    let path = path(node, id, input);
    fastctx_platform::ensure_private_dir(path.parent().expect("progress directory"))?;
    fastctx_platform::publish(&path, &serde_json::to_vec(progress)?, fastctx_platform::Permissions::Private)
}

/// Preflight is durable before Waking is published. A restart can distinguish
/// a request never offered to the CLI from an unanswered launch.
pub(crate) fn begin(node: &Node, id: &ResidentId, input: &NoticeId) -> Result<(), Failure> {
    let _transaction = crate::resident_storage::lock();
    if progress(node, id, input).map_err(failure)?.is_none() {
        let now = Timestamp::now();
        keep(
            node,
            id,
            input,
            &Progress {
                began: now,
                next: now,
                failures: 0,
                reason: None,
                attempted: false,
                accepted: false,
                after_activation: None,
            },
        )
        .map_err(failure)?;
    }
    Ok(())
}

/// The next activation, not a late report from the previous one, must consume
/// this wake. Commit that boundary before publishing the Waking state.
pub(crate) fn begin_after(
    node: &Node,
    id: &ResidentId,
    input: &NoticeId,
    activation: Option<String>,
) -> Result<(), Failure> {
    begin(node, id, input)?;
    let _transaction = crate::resident_storage::lock();
    let mut kept = progress(node, id, input).map_err(failure)?.ok_or_else(|| failure("missing host preflight"))?;
    kept.after_activation = activation;
    keep(node, id, input, &kept).map_err(failure)
}

/// Commits the boundary before a command capable of accepting the input runs.
pub(crate) fn attempting(node: &Node, id: &ResidentId, input: &NoticeId) -> Result<(), Failure> {
    let _transaction = crate::resident_storage::lock();
    let mut kept = progress(node, id, input).map_err(failure)?.ok_or_else(|| failure("missing host preflight"))?;
    kept.attempted = true;
    kept.accepted = false;
    keep(node, id, input, &kept).map_err(failure)
}

pub(crate) fn accepted(node: &Node, id: &ResidentId, input: &NoticeId) -> Result<(), Failure> {
    let _transaction = crate::resident_storage::lock();
    let Some(mut kept) = progress(node, id, input).map_err(failure)? else { return Ok(()) };
    kept.accepted = true;
    keep(node, id, input, &kept).map_err(failure)
}

pub(crate) async fn unavailable(
    node: &Arc<Node>,
    id: &ResidentId,
    input: &NoticeId,
    error: super::wake::WakeFailure,
) -> Result<Landed, Failure> {
    if error.permanent {
        super::refused::reject(node, id, input, error.failure).await
    } else {
        defer(node, id, input, &error.failure.message)
    }
}

/// The caller owns the action and management gates and retries its durable work.
/// Its scheduler may check sooner, but only this progress permits another wake.
pub(crate) async fn prepare_work(node: &Arc<Node>, id: &ResidentId, key: &NoticeId) -> Result<(), Failure> {
    if progress(node, id, key).map_err(failure)?.is_some_and(|progress| progress.next > Timestamp::now()) {
        return Err(Failure::host_unavailable("the host retry is waiting for its next attempt"));
    }
    match super::wake::wake(node, id, super::wake::Source::Control, None).await {
        Ok(_) => clear(node, id, key).map_err(failure),
        Err(error) => {
            if !error.permanent {
                failed(node, id, key, &error.failure.message)?;
            } else {
                clear(node, id, key).map_err(failure)?;
            }
            Err(error.control())
        }
    }
}

/// Call only with evidence that this input has not crossed the host boundary.
pub(crate) fn defer(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<Landed, Failure> {
    failed(node, id, input, reason)?;
    crate::inbox::set_state(&node.world.conversation(id), input, PendingState::Waiting).map_err(failure)?;
    kick(node, id);
    Ok(Landed::Waiting { why: None, answered: true })
}

pub(crate) fn failed(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<(), Failure> {
    begin(node, id, input)?;
    {
        let _transaction = crate::resident_storage::lock();
        let mut kept = progress(node, id, input).map_err(failure)?.ok_or_else(|| failure("missing host progress"))?;
        if kept.reason.as_deref() != Some(reason) {
            node.log.line(format_args!("lifecycle {id}: host unavailable; retrying: {reason}"));
        }
        kept.reason = Some(reason.into());
        kept.attempted = false;
        kept.accepted = false;
        kept.failures = kept.failures.saturating_add(1);
        let seconds = 3u64.saturating_mul(1u64 << kept.failures.saturating_sub(1).min(5)).min(60);
        kept.next = Timestamp::now().checked_add(Duration::from_secs(seconds)).map_err(failure)?;
        keep(node, id, input, &kept).map_err(failure)?;
    }
    Ok(())
}

/// Records a startup observation without granting permission to resubmit.
pub(crate) fn observing(node: &Node, id: &ResidentId, input: &NoticeId, reason: String) -> Result<(), Failure> {
    begin(node, id, input)?;
    let _transaction = crate::resident_storage::lock();
    let mut kept = progress(node, id, input).map_err(failure)?.ok_or_else(|| failure("missing host progress"))?;
    if kept.reason.as_ref() != Some(&reason) {
        node.log.line(format_args!("lifecycle {id}: observing startup: {reason}"));
        kept.reason = Some(reason);
        keep(node, id, input, &kept).map_err(failure)?;
    }
    Ok(())
}

pub(crate) fn clear(node: &Node, id: &ResidentId, input: &NoticeId) -> io::Result<()> {
    match std::fs::remove_file(path(node, id, input)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

pub(crate) fn kick(node: &Arc<Node>, id: &ResidentId) {
    let signal = {
        let mut workers = node.retrying.0.lock().expect("never poisoned");
        let std::collections::hash_map::Entry::Vacant(entry) = workers.entry(id.clone()) else { return };
        let signal = Arc::new(tokio::sync::Notify::new());
        entry.insert(signal.clone());
        signal
    };
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(run(node, id, signal));
}

/// Host readiness and consuming-turn receipts release waiting arrivals
/// immediately. A retry request itself does not notify its own worker.
pub(crate) fn changed(node: &Node, id: &ResidentId) {
    if let Some(signal) = node.retrying.0.lock().expect("never poisoned").get(id) {
        signal.notify_one();
    }
}

fn run(
    node: Arc<Node>,
    id: ResidentId,
    signal: Arc<tokio::sync::Notify>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move {
        node.ready().await;
        let mut trouble = None;
        loop {
            if node.residents.active(&id).is_err() {
                break;
            }
            let Some((about, _)) = node.residents.about(&id) else { break };
            let directory = node.world.conversation(&id);
            let next = crate::inbox::pending(&directory).map(|rows| {
                rows.into_iter().find(|row| {
                    row.state == PendingState::Waiting
                        && row.delivery.notice.along().is_none()
                        && crate::speech::ready_for_delivery(&node, &about, row)
                })
            });
            let input = match next {
                Ok(Some(input)) => input,
                Ok(None) => match crate::control_queue::waiting_for_host(&node, &id) {
                    Ok(true) => {
                        if let Err(error) = crate::control_queue::run_pending(&node, &id).await {
                            node.log.line(format_args!("lifecycle {id}: retrying control: {error}"));
                        }
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        continue;
                    }
                    Ok(false) => break,
                    Err(error) => {
                        node.log.line(format_args!("lifecycle {id}: reading pending controls: {error}"));
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        continue;
                    }
                },
                Err(error) => {
                    let error = error.to_string();
                    if trouble.as_ref() != Some(&error) {
                        node.log.line(format_args!("lifecycle {id}: reading pending work; retrying: {error}"));
                        trouble = Some(error);
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            };
            match progress(&node, &id, &input.delivery.id) {
                Ok(Some(progress)) => {
                    let remaining = progress.next.as_millisecond().saturating_sub(Timestamp::now().as_millisecond());
                    if remaining > 0 {
                        tokio::time::sleep(Duration::from_millis(remaining.min(60_000) as u64)).await;
                        continue;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    node.log.line(format_args!("lifecycle {id}: cannot read host progress: {error}"));
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            }
            let Some((about, _)) = node.residents.about(&id) else { break };
            if let Err(error) = crate::speech::hand_over(&node, &id, &about, &directory, input.clone()).await {
                // A failure after handover is not permission to repeat it.
                // Only the still-Waiting state proves no submission happened.
                if crate::inbox::read(&directory, &input.delivery.id)
                    .is_ok_and(|row| row.is_some_and(|row| row.state == PendingState::Waiting))
                {
                    if let Err(error) = defer(&node, &id, &input.delivery.id, &error.message) {
                        node.log.line(format_args!("lifecycle {id}: keeping retry: {error}"));
                    }
                } else {
                    node.log.line(format_args!("lifecycle {id}: handover needs reconciliation: {error}"));
                }
            }
            if crate::inbox::read(&directory, &input.delivery.id)
                .is_ok_and(|row| row.is_some_and(|row| row.state == PendingState::Waiting))
            {
                tokio::select! {
                    () = signal.notified() => {},
                    () = tokio::time::sleep(Duration::from_secs(3)) => {},
                }
            }
        }
        node.retrying.0.lock().expect("never poisoned").remove(&id);
        // An arrival racing the final empty read either starts its own worker
        // after this removal, or is picked up by this recheck.
        if node.residents.active(&id).is_ok()
            && (crate::inbox::pending(&node.world.conversation(&id)).is_ok_and(|rows| {
                node.residents.about(&id).is_some_and(|(about, _)| {
                    rows.iter().any(|row| {
                        row.state == PendingState::Waiting
                            && row.delivery.notice.along().is_none()
                            && crate::speech::ready_for_delivery(&node, &about, row)
                    })
                })
            }) || crate::control_queue::waiting_for_host(&node, &id).unwrap_or(false))
        {
            kick(&node, &id);
        }
    })
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("keeping host progress: {error}"))
}
