//! Durable admission is independent of the work needed to deliver a letter.
//! Each conversation drains its own queue; a deferred completion yields to
//! later letters, including the enclosing answer that may consume it.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::records::items;
use fastctx_types::ResidentId;
use fastctx_types::residents::{Delivery, Notice};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::Node;
use crate::speech::Owed;

#[derive(Default)]
pub(crate) struct Received {
    workers: Mutex<HashMap<ResidentId, Worker>>,
}

struct Worker {
    wake: Arc<Notify>,
    dirty: bool,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Entry {
    to: ResidentId,
    delivery: Delivery,
    owed: Owed,
    #[serde(default)]
    order: u64,
    #[serde(default)]
    decision: crate::completion_delivery::Progress,
    #[serde(default)]
    released: Option<bool>,
}

fn directory(node: &Node, to: &ResidentId) -> PathBuf {
    node.world.conversation(to).book("received")
}

/// Returns only after this node can recover the accepted letter on restart.
pub(crate) fn keep(node: &Arc<Node>, to: &ResidentId, delivery: Delivery, owed: Owed) -> io::Result<()> {
    if !node.residents.knows(to) {
        if node.residents.deleted(to).is_some() {
            return Ok(());
        }
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no receiving conversation {to}")));
    }
    let directory = directory(node, to);
    let path = directory.join(format!("{}.json", delivery.id));
    let at = delivery.at;
    let entry = Entry { to: to.clone(), delivery, owed, order: 0, decision: Default::default(), released: None };
    match items::enqueue(&path, serde_json::to_vec(&entry)?, Some(at))? {
        items::Enqueued::Inserted => {}
        items::Enqueued::Finished => return Ok(()),
        items::Enqueued::Existing(bytes) => {
            let prior: Entry = serde_json::from_slice(&bytes)?;
            if prior.to != entry.to || prior.delivery.notice != entry.delivery.notice || prior.owed != entry.owed {
                return Err(io::Error::other("the accepted letter changed under the same identity"));
            }
        }
    }
    wake(node, to);
    Ok(())
}

pub(crate) fn wake(node: &Arc<Node>, to: &ResidentId) {
    let mut workers = node.received.workers.lock().expect("never poisoned");
    if let Some(worker) = workers.get_mut(to) {
        worker.dirty = true;
        worker.wake.notify_one();
        return;
    }
    let wake = Arc::new(Notify::new());
    workers.insert(to.clone(), Worker { wake: wake.clone(), dirty: false });
    let (node, to) = (node.clone(), to.clone());
    tokio::spawn(async move { drain(node, to, wake).await });
}

pub(crate) fn resume(node: &Arc<Node>) {
    match node.world.kept() {
        Ok(kept) => {
            for kept in kept {
                if let fastctx_types::layout::Kind::Home(to) = kept.kind {
                    wake(node, &to);
                }
            }
        }
        Err(error) => node.log.line(format_args!("finding received letters: {error}")),
    }
}

async fn drain(node: Arc<Node>, to: ResidentId, wake: Arc<Notify>) {
    let mut ways = node.ways.subscribe();
    let mut errors = HashMap::new();
    let mut again = crate::again::Again::default();
    let (mut after, mut key) = (Vec::new(), String::new());
    let mut through = Vec::new();
    let mut pause = fastctx_net::again::LONGEST;
    let mut failed = false;
    loop {
        let notified = wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        ways.borrow_and_update();
        if after.is_empty()
            && let Some(worker) = node.received.workers.lock().expect("received workers").get_mut(&to)
        {
            worker.dirty = false;
        }
        if after.is_empty() {
            let folder = directory(&node, &to);
            match tokio::task::spawn_blocking(move || items::queue_end(&folder)).await {
                Ok(Ok(end)) => through = end,
                other => {
                    node.log.line(format_args!("reading queue boundary: {other:?}"));
                    tokio::time::sleep(again.next()).await;
                    continue;
                }
            }
        }
        let upper = through.clone();
        let (folder, cursor, last) = (directory(&node, &to), after.clone(), key.clone());
        let loaded = tokio::task::spawn_blocking(move || {
            items::queued(&folder, &cursor, &last, &upper).map(|rows| {
                rows.into_iter()
                    .map(|row| {
                        let parsed = serde_json::from_slice::<Entry>(&row.bytes).map_err(io::Error::other);
                        (row.order, row.path, parsed)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .await
        .map_err(io::Error::other)
        .and_then(|result| result);
        let mut more = false;
        match loaded {
            Ok(rows) => {
                more = !rows.is_empty();
                for (order, path, parsed) in rows {
                    after = order;
                    key = path.file_name().and_then(|name| name.to_str()).expect("queue identity").to_string();
                    let mut entry: Entry = match parsed {
                        Ok(entry) => entry,
                        Err(error) => {
                            failed = true;
                            report(&node, &mut errors, &path, &io::Error::other(error));
                            continue;
                        }
                    };
                    let result = deliver(&node, &mut entry).await;
                    let due = entry.decision.due;
                    let waiting = matches!(result, Ok(Handover::Waiting));
                    let (working, record) = (node.clone(), path.clone());
                    let saved = tokio::task::spawn_blocking(move || -> io::Result<()> {
                        match result {
                            Ok(Handover::Done) => items::delete_if_exists(&record),
                            Ok(Handover::Waiting) => items::keep(&record, &entry),
                            Ok(Handover::Refused(reason)) => {
                                let bytes = items::read(&record)?;
                                crate::obligations::finish_record(
                                    &working,
                                    &record,
                                    &bytes,
                                    crate::obligations::Outcome::Impossible,
                                    &reason,
                                )
                                .map(|_| ())
                            }
                            Err(error) => {
                                items::keep(&record, &entry)?;
                                Err(error)
                            }
                        }
                    })
                    .await
                    .map_err(io::Error::other)
                    .and_then(|result| result);
                    if let Err(error) = saved {
                        failed = true;
                        report(&node, &mut errors, &path, &error);
                    } else {
                        errors.remove(&path);
                    }
                    if waiting && let Some(due) = due {
                        pause = pause.min(Duration::from_millis(
                            due.as_millisecond().saturating_sub(Timestamp::now().as_millisecond()).max(0) as u64,
                        ));
                    }
                }
            }
            Err(error) => {
                failed = true;
                report(&node, &mut errors, &directory(&node, &to), &error);
            }
        }
        if more {
            tokio::task::yield_now().await;
            continue;
        }
        after.clear();
        key.clear();
        let folder = directory(&node, &to);
        let empty =
            tokio::task::spawn_blocking(move || items::page(&folder, Some("json"), "", 1).map(|rows| rows.is_empty()))
                .await
                .is_ok_and(|result| result.is_ok_and(|empty| empty));
        let dirty = {
            let mut workers = node.received.workers.lock().expect("received workers");
            let dirty = workers.get(&to).is_some_and(|worker| worker.dirty);
            if empty && !dirty {
                workers.remove(&to);
                return;
            }
            dirty
        };
        if dirty {
            continue;
        }
        if failed || !errors.is_empty() {
            pause = pause.min(again.next());
        } else {
            again.reset();
        }
        let waiting = pause;
        pause = fastctx_net::again::LONGEST;
        failed = false;
        tokio::select! {()=tokio::time::sleep(waiting)=>{},()=&mut notified=>{},_=ways.changed()=>{}}
    }
}

fn report(node: &Node, errors: &mut HashMap<PathBuf, String>, path: &Path, error: &io::Error) {
    let reason = error.to_string();
    if errors.get(path) != Some(&reason) {
        node.log.line(format_args!("processing received letter {}: {reason}", path.display()));
        errors.insert(path.to_path_buf(), reason);
    }
}

enum Handover {
    Waiting,
    Done,
    Refused(String),
}

async fn deliver(node: &Arc<Node>, entry: &mut Entry) -> io::Result<Handover> {
    if node.residents.deleted(&entry.to).is_some() {
        return Ok(Handover::Refused("the receiving conversation was deleted".into()));
    }
    let result = if let Notice::JobEnded { consumed, .. } = &entry.delivery.notice {
        let (directory, consumed) = (node.world.conversation(&entry.to), consumed.clone());
        tokio::task::spawn_blocking(move || crate::job_receipts::apply_consumed(&directory, &consumed))
            .await
            .map_err(io::Error::other)??;
        if entry.released.is_none() {
            match crate::completion_delivery::decide(node, &entry.to, &entry.delivery, &mut entry.decision).await? {
                crate::completion_delivery::Decision::Consumed => return Ok(Handover::Done),
                crate::completion_delivery::Decision::Held => return Ok(Handover::Waiting),
                crate::completion_delivery::Decision::Deliver { unconfirmed } => entry.released = Some(unconfirmed),
            }
        }
        let mut delivery = entry.delivery.clone();
        if let Notice::JobEnded { follower_unconfirmed, .. } = &mut delivery.notice {
            *follower_unconfirmed = entry.released == Some(true);
        }
        crate::speech::tell_completion(node, &entry.to, delivery).await
    } else {
        if matches!(
            entry.delivery.notice,
            Notice::Words { goal_report: None, goal_question: None, .. } | Notice::Attached { .. }
        ) && (node.residents.is_deleting(&entry.to) || node.residents.removed(&entry.to).is_some())
        {
            return Ok(Handover::Refused("the receiving conversation no longer accepts words".into()));
        }
        crate::speech::tell_here(node, &entry.to, entry.delivery.clone(), entry.owed.clone()).await
    };
    match result {
        Ok(_) => Ok(Handover::Done),
        Err(error) if crate::obligations::permanent(&error) || crate::lifecycle::refused::reported(&error) => {
            Ok(Handover::Refused(error.message))
        }
        Err(error) => Err(io::Error::other(error.message)),
    }
}
