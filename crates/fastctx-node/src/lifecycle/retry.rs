//! Durable progress before a host accepts work. Waiting never owns a gate.

use crate::Node;
use fastctx_types::{
    NoticeId, ResidentId,
    ipc::Failure,
    layout::ConversationDir,
    residents::{About, Landed, Pending, PendingState},
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io,
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

pub(crate) fn directory(node: &Node, id: &ResidentId) -> io::Result<fastctx_types::layout::ConversationDir> {
    let directory = node.world.conversation(id);
    crate::records::legacy::directory::<Progress>(&directory, "host-progress", &directory.book("host-progress"))?;
    Ok(directory)
}
pub(crate) fn progress_now(node: &Node, id: &ResidentId, input: &NoticeId) -> io::Result<Option<Progress>> {
    let directory = directory(node, id)?;
    crate::records::read(&directory, |rows| rows.get("host-progress", input.as_str()))
}
/// `progress_now`, read where reading may block.
pub(crate) async fn progress(node: &Arc<Node>, id: &ResidentId, input: &NoticeId) -> io::Result<Option<Progress>> {
    let (node, id, input) = (node.clone(), id.clone(), input.clone());
    fastctx_platform::task::blocking(move || progress_now(&node, &id, &input)).await
}
fn initial() -> Progress {
    let now = Timestamp::now();
    Progress {
        began: now,
        next: now,
        failures: 0,
        reason: None,
        attempted: false,
        accepted: false,
        after_activation: None,
    }
}
fn update<T: Send + 'static>(
    node: &Node,
    id: &ResidentId,
    input: &NoticeId,
    create: bool,
    apply: impl FnOnce(&mut Progress) -> fastctx_storage::Result<T> + Send + 'static,
) -> Result<Option<T>, Failure> {
    let directory = directory(node, id).map_err(failure)?;
    let key = input.to_string();
    crate::records::write_at(&directory, fastctx_storage::Priority::Interactive, 2048, move |rows| {
        let mut progress = match rows.get::<Progress>("host-progress", &key)? {
            Some(row) => row,
            None if create => initial(),
            None => return Ok(None),
        };
        let result = apply(&mut progress)?;
        rows.put("host-progress", &key, &progress)?;
        Ok(Some(result))
    })
    .map_err(failure)
}
/// Preflight is durable before Waking is published.
pub(crate) fn begin(node: &Node, id: &ResidentId, input: &NoticeId) -> Result<(), Failure> {
    update(node, id, input, true, |_| Ok(())).map(|_| ())
}
pub(crate) fn begin_after(
    node: &Node,
    id: &ResidentId,
    input: &NoticeId,
    activation: Option<String>,
) -> Result<(), Failure> {
    update(node, id, input, true, move |row| {
        row.after_activation = activation;
        Ok(())
    })
    .map(|_| ())
}
/// Commit the host boundary before issuing a command that could accept words.
pub(crate) fn attempting(node: &Node, id: &ResidentId, input: &NoticeId) -> Result<(), Failure> {
    update(node, id, input, false, |row| {
        row.attempted = true;
        row.accepted = false;
        Ok(())
    })?
    .ok_or_else(|| failure("missing host preflight"))
}
pub(crate) fn accepted(node: &Node, id: &ResidentId, input: &NoticeId) -> Result<(), Failure> {
    update(node, id, input, false, |row| {
        row.accepted = true;
        Ok(())
    })
    .map(|_| ())
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
        defer(node, id, input, &error.failure.message).await
    }
}

/// The caller owns the action and management gates and retries its durable work.
/// Its scheduler may check sooner, but only this progress permits another wake.
pub(crate) async fn prepare_work(node: &Arc<Node>, id: &ResidentId, key: &NoticeId) -> Result<(), Failure> {
    if progress(node, id, key).await.map_err(failure)?.is_some_and(|progress| progress.next > Timestamp::now()) {
        return Err(Failure::host_unavailable("the host retry is waiting for its next attempt"));
    }
    match super::wake::wake(node, id, super::wake::Source::Control, None).await {
        Ok(_) => clear(node, id, key).await.map_err(failure),
        Err(error) => {
            if !error.permanent {
                failed(node, id, key, &error.failure.message).await?;
            } else {
                clear(node, id, key).await.map_err(failure)?;
            }
            Err(error.control())
        }
    }
}

/// Call only with evidence that this input has not crossed the host boundary.
pub(crate) fn defer_now(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<Landed, Failure> {
    failed_now(node, id, input, reason)?;
    crate::inbox::set_state(&node.world.conversation(id), input, PendingState::Waiting).map_err(failure)?;
    kick(node, id);
    Ok(Landed::Waiting { why: None, answered: true })
}
/// `defer_now`, written where writing may block.
pub(crate) async fn defer(
    node: &Arc<Node>,
    id: &ResidentId,
    input: &NoticeId,
    reason: &str,
) -> Result<Landed, Failure> {
    let (node, id, input, reason) = (node.clone(), id.clone(), input.clone(), reason.to_string());
    fastctx_platform::task::blocking(move || defer_now(&node, &id, &input, &reason)).await
}

/// `failed_now`, written where writing may block.
pub(crate) async fn failed(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<(), Failure> {
    let (node, id, input, reason) = (node.clone(), id.clone(), input.clone(), reason.to_string());
    fastctx_platform::task::blocking(move || failed_now(&node, &id, &input, &reason)).await
}

pub(crate) fn failed_now(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<(), Failure> {
    let why = reason.to_string();
    let changed = update(node, id, input, true, move |row| {
        let changed = row.reason.as_deref() != Some(&why);
        row.reason = Some(why);
        row.attempted = false;
        row.accepted = false;
        row.failures = row.failures.saturating_add(1);
        let seconds = 3u64.saturating_mul(1u64 << row.failures.saturating_sub(1).min(5)).min(60);
        row.next = Timestamp::now()
            .checked_add(Duration::from_secs(seconds))
            .map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
        Ok(changed)
    })?
    .unwrap_or(false);
    if changed {
        node.log.line(format_args!("lifecycle {id}: host unavailable; retrying: {reason}"));
    }
    Ok(())
}
/// `queued_now`, written where writing may block.
pub(crate) async fn queued(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<(), Failure> {
    let (node, id, input, reason) = (node.clone(), id.clone(), input.clone(), reason.to_string());
    fastctx_platform::task::blocking(move || queued_now(&node, &id, &input, &reason)).await
}
pub(crate) fn queued_now(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<(), Failure> {
    let why = reason.to_string();
    update(node, id, input, true, move |row| {
        row.reason = Some(why);
        row.attempted = false;
        row.accepted = false;
        row.next = Timestamp::now()
            .checked_add(Duration::from_secs(2))
            .map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
        Ok(())
    })
    .map(|_| ())
}
pub(crate) fn observing(node: &Node, id: &ResidentId, input: &NoticeId, reason: String) -> Result<(), Failure> {
    let why = reason.clone();
    let changed = update(node, id, input, true, move |row| {
        let changed = row.reason.as_ref() != Some(&why);
        if changed {
            row.reason = Some(why);
        }
        Ok(changed)
    })?
    .unwrap_or(false);
    if changed {
        node.log.line(format_args!("lifecycle {id}: observing startup: {reason}"));
    }
    Ok(())
}
/// `clear_now`, written where writing may block.
pub(crate) async fn clear(node: &Arc<Node>, id: &ResidentId, input: &NoticeId) -> io::Result<()> {
    let (node, id, input) = (node.clone(), id.clone(), input.clone());
    fastctx_platform::task::blocking(move || clear_now(&node, &id, &input)).await
}
pub(crate) fn clear_now(node: &Node, id: &ResidentId, input: &NoticeId) -> io::Result<()> {
    let directory = directory(node, id)?;
    let key = input.to_string();
    crate::records::write_at(&directory, fastctx_storage::Priority::Interactive, key.len(), move |rows| {
        rows.delete("host-progress", &key).map(|_| ())
    })
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
    fastctx_platform::task::spawn(run(node, id, signal));
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
        node.ready_for(&id).await;
        let mut trouble = None;
        loop {
            if node.residents.active(&id).is_err() {
                break;
            }
            let Some((about, _)) = node.residents.about(&id) else { break };
            let directory = node.world.conversation(&id);
            let input = match first_ready(&node, &about, &directory).await {
                Ok(Some(input)) => input,
                Ok(None) => match crate::control_queue::waiting_for_host(&node, &id).await {
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
            match progress(&node, &id, &input.delivery.id).await {
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
                if still_waiting(&directory, &input.delivery.id).await {
                    if let Err(error) = defer(&node, &id, &input.delivery.id, &error.message).await {
                        node.log.line(format_args!("lifecycle {id}: keeping retry: {error}"));
                    }
                } else {
                    node.log.line(format_args!("lifecycle {id}: handover needs reconciliation: {error}"));
                }
            }
            if still_waiting(&directory, &input.delivery.id).await {
                tokio::select! {
                    () = signal.notified() => {},
                    () = tokio::time::sleep(Duration::from_secs(3)) => {},
                }
            }
        }
        node.retrying.0.lock().expect("never poisoned").remove(&id);
        // An arrival racing the final empty read either starts its own worker
        // after this removal, or is picked up by this recheck.
        if node.residents.active(&id).is_ok() {
            let ready = match node.residents.about(&id) {
                Some((about, _)) => {
                    first_ready(&node, &about, &node.world.conversation(&id)).await.is_ok_and(|first| first.is_some())
                }
                None => false,
            };
            if ready || crate::control_queue::waiting_for_host(&node, &id).await.unwrap_or(false) {
                kick(&node, &id);
            }
        }
    })
}

/// The first of what waits in `directory` that may go in now, read where
/// reading may block.
async fn first_ready(node: &Arc<Node>, about: &About, directory: &ConversationDir) -> io::Result<Option<Pending>> {
    let (node, about, directory) = (node.clone(), about.clone(), directory.clone());
    fastctx_platform::task::blocking(move || {
        crate::inbox::pending(&directory).map(|rows| {
            rows.into_iter().find(|row| {
                row.state == PendingState::Waiting
                    && row.delivery.notice.along().is_none()
                    && crate::speech::ready_for_delivery(&node, &about, row)
            })
        })
    })
    .await
}

/// Whether `input` still waits to go in: only that proves it was not handed
/// over, so only then is it put off for later.
pub(crate) async fn still_waiting(directory: &ConversationDir, input: &NoticeId) -> bool {
    let (directory, input) = (directory.clone(), input.clone());
    fastctx_platform::task::blocking(move || {
        crate::inbox::read(&directory, &input)
            .is_ok_and(|row| row.is_some_and(|row| row.state == PendingState::Waiting))
    })
    .await
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("keeping host progress: {error}"))
}
