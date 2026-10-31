//! Durable admission is independent of the work needed to deliver a letter.
//! Each conversation drains its own queue; a deferred completion yields to
//! later letters, including the enclosing answer that may consume it.

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::ResidentId;
use fastctx_types::residents::{Delivery, Notice};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::Node;
use crate::speech::Owed;

#[derive(Default)]
pub(crate) struct Received {
    /// Each conversation's worker; whoever keeps a letter wakes it, or
    /// starts one when none runs.
    workers: Mutex<HashMap<ResidentId, Arc<Notify>>>,
    /// A conversation's letters are numbered and kept one at a time, under
    /// the one of these its id falls to; other conversations' go on.
    keeping: [Mutex<()>; 16],
}

impl Received {
    fn keeping(&self, to: &ResidentId) -> &Mutex<()> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        to.hash(&mut hasher);
        &self.keeping[hasher.finish() as usize % self.keeping.len()]
    }
}

#[derive(Serialize, Deserialize)]
struct Entry {
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
    ensure_private_dir(&directory)?;
    let path = directory.join(format!("{}.json", delivery.id));
    // A persisted sequence preserves arrival order even if the clock moves
    // back. The letter is kept before its worker is woken, so a worker
    // either finds it before it retires or a new one starts after.
    let publishing = node.received.keeping(to).lock().expect("never poisoned");
    let sequence = directory.join("sequence");
    let order = match std::fs::read_to_string(&sequence) {
        Ok(text) => text.parse::<u64>().map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let next = order.checked_add(1).ok_or_else(|| io::Error::other("received letter sequence exhausted"))?;
    publish(&sequence, next.to_string().as_bytes(), Permissions::Private)?;
    let entry = Entry { to: to.clone(), delivery, owed, order, decision: Default::default(), released: None };
    let bytes = serde_json::to_vec(&entry).map_err(io::Error::other)?;
    match publish_new(&path, &bytes, Permissions::Private) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let prior: Entry = serde_json::from_slice(&std::fs::read(&path)?).map_err(io::Error::other)?;
            if prior.to != entry.to || prior.delivery.notice != entry.delivery.notice || prior.owed != entry.owed {
                return Err(io::Error::other("the accepted letter changed under the same identity"));
            }
        }
        result => result?,
    }
    drop(publishing);
    wake(node, to);
    Ok(())
}

pub(crate) fn wake(node: &Arc<Node>, to: &ResidentId) {
    let mut workers = node.received.workers.lock().expect("never poisoned");
    if let Some(wake) = workers.get(to) {
        wake.notify_one();
        return;
    }
    let wake = Arc::new(Notify::new());
    workers.insert(to.clone(), wake.clone());
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

fn paths(directory: &Path) -> io::Result<Vec<PathBuf>> {
    Ok(crate::retention::entries(directory)?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .filter(|path| !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')))
        .collect())
}

async fn drain(node: Arc<Node>, to: ResidentId, wake: Arc<Notify>) {
    let mut ways = node.ways.subscribe();
    let mut errors = HashMap::new();
    let mut again = crate::again::Again::default();
    // The letters read so far, by path: each wake lists the folder and reads
    // only the letters new to it, so a backlog is read once, not once a wake.
    let mut known: HashMap<PathBuf, Entry> = HashMap::new();
    loop {
        let notified = wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        ways.borrow_and_update();
        match paths(&directory(&node, &to)) {
            Ok(paths) => {
                let listed: std::collections::HashSet<&PathBuf> = paths.iter().collect();
                known.retain(|path, _| listed.contains(path));
                for path in paths {
                    if known.contains_key(&path) {
                        continue;
                    }
                    match fastctx_install::renewal::read_json::<Entry>(&path) {
                        Ok(Some(entry)) => {
                            known.insert(path, entry);
                        }
                        Ok(None) => {}
                        Err(error) => report(&node, &mut errors, &path, &error),
                    }
                }
            }
            Err(error) => report(&node, &mut errors, &directory(&node, &to), &error),
        }
        let mut waiting: Vec<(PathBuf, u64, Timestamp)> =
            known.iter().map(|(path, entry)| (path.clone(), entry.order, entry.delivery.at)).collect();
        waiting.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
        let mut pause = fastctx_net::again::LONGEST;
        let mut failed = false;
        for (path, _, _) in waiting {
            let Some(mut entry) = known.remove(&path) else { continue };
            match deliver(&node, &mut entry).await {
                Ok(outcome) => {
                    let done = !matches!(outcome, Handover::Waiting);
                    let result = match outcome {
                        Handover::Done => std::fs::remove_file(&path),
                        Handover::Waiting => fastctx_install::renewal::write_json(&path, &entry),
                        Handover::Refused(reason) => std::fs::read(&path).and_then(|bytes| {
                            crate::obligations::finish(
                                &node,
                                &path,
                                &bytes,
                                crate::obligations::Outcome::Impossible,
                                &reason,
                            )
                            .map(|_| ())
                        }),
                    };
                    let stored = result.is_ok();
                    if let Err(error) = result {
                        failed = true;
                        report(&node, &mut errors, &path, &error);
                    } else {
                        errors.remove(&path);
                    }
                    if !done && let Some(due) = entry.decision.due {
                        let millis = due.as_millisecond().saturating_sub(Timestamp::now().as_millisecond()).max(0);
                        pause = pause.min(Duration::from_millis(millis as u64));
                    }
                    // A letter still waiting stays known as it was just kept;
                    // one whose keeping failed is read again from disk.
                    if !done && stored {
                        known.insert(path, entry);
                    }
                }
                Err(error) => {
                    failed = true;
                    match fastctx_install::renewal::write_json(&path, &entry) {
                        Ok(()) => {
                            known.insert(path.clone(), entry);
                        }
                        Err(storing) => report(&node, &mut errors, &path, &storing),
                    }
                    report(&node, &mut errors, &path, &error);
                }
            }
        }
        // Recheck under the registry lock: an enqueue either precedes this
        // read or starts a new worker after this one removes itself.
        {
            let mut workers = node.received.workers.lock().expect("never poisoned");
            if paths(&directory(&node, &to)).is_ok_and(|paths| paths.is_empty()) {
                workers.remove(&to);
                return;
            }
        }
        if failed || !errors.is_empty() {
            pause = pause.min(again.next());
        } else {
            again.reset();
        }
        tokio::select! {
            () = tokio::time::sleep(pause) => {}
            () = &mut notified => {}
            _ = ways.changed() => {}
        }
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
        crate::job_receipts::apply_consumed(&node.world.conversation(&entry.to), consumed)?;
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
