//! Admitted control mail is durable before network acknowledgement. Work is
//! serialized per owner, including guest owners with no host on this node.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::records::items;
use fastctx_net::link::Caller;
use fastctx_types::access::{HubAccount, HubId};
use fastctx_types::delivery_control::ControlLetter;
use fastctx_types::ipc::Failure;
use fastctx_types::{Address, NoticeId};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::Node;
use crate::obligations::Outcome;

#[derive(Default)]
pub(crate) struct Receiving {
    workers: Mutex<HashMap<Address, Worker>>,
    /// The controls being applied, by the path their mail is kept at.
    applying: Mutex<HashMap<PathBuf, Applied>>,
    pub(crate) changed: Notify,
}

/// What became of a control's application, once it ended.
type Applied = tokio::sync::watch::Receiver<Option<Result<(), Failure>>>;

struct Worker {
    wake: Arc<Notify>,
    dirty: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Entry {
    scope: HubAccount,
    command: ControlLetter,
    order: u64,
    from: fastctx_types::MachineName,
    request: NoticeId,
    #[serde(default)]
    result: Option<Result<(), Failure>>,
    #[serde(default)]
    posted: Option<crate::letters::Posted>,
}

pub(crate) fn admit(node: &Arc<Node>, hub: &HubId, caller: &Caller, command: &ControlLetter) -> Result<(), Failure> {
    let link = node.link_of(hub).ok_or_else(|| Failure::new("the control's hub is no longer connected"))?;
    if let ControlLetter::CompletionReceipt { owner, origin, .. } = command {
        let valid = match origin {
            Some(origin) => origin.resident == *owner,
            None => link.view().same(&owner.machine, &caller.machine),
        };
        if !valid {
            return Err(Failure::refused("the completion receipt names another owner"));
        }
    }
    if let ControlLetter::StopRun { owner, .. } = command
        && !link.view().same(&owner.machine, &caller.machine)
    {
        return Err(Failure::refused("only the run owner's machine can stop its dispatch"));
    }
    crate::admission::admit(node, &link, caller, &command.call()).map(|_| ())
}

fn directory(node: &Node, owner: &Address) -> PathBuf {
    node.world.owner(&node.names(), owner).book("received-controls")
}

pub(crate) fn keep(
    node: &Arc<Node>,
    hub: &HubId,
    caller: &Caller,
    identity: &crate::letters::Identity,
    mut command: ControlLetter,
    refused: Option<Failure>,
) -> io::Result<()> {
    let scope = HubAccount { hub: hub.clone(), account: caller.account.clone() };
    match &mut command {
        ControlLetter::CompletionReceipt { origin: Some(origin), .. } | ControlLetter::StopOn { origin, .. } => {
            origin.account = Some(scope.clone());
        }
        _ => {}
    }
    let owner = command.owner().clone();
    let directory = directory(node, &owner);
    let path = directory.join(format!("{}.json", identity.id));
    let entry = Entry {
        scope,
        command,
        order: 0,
        from: caller.machine.clone(),
        request: identity.original_id().parse().map_err(io::Error::other)?,
        result: refused.map(Err),
        posted: None,
    };
    items::enqueue(&path, serde_json::to_vec(&entry)?, None)?;
    wake(node, &owner);
    Ok(())
}

fn wake(node: &Arc<Node>, owner: &Address) {
    let mut workers = node.control_letters.workers.lock().expect("never poisoned");
    if let Some(worker) = workers.get_mut(owner) {
        worker.dirty = true;
        worker.wake.notify_one();
        return;
    }
    let wake = Arc::new(Notify::new());
    workers.insert(owner.clone(), Worker { wake: wake.clone(), dirty: false });
    let (node, owner) = (node.clone(), owner.clone());
    tokio::spawn(async move { drain(node, owner, wake).await });
}

pub(crate) fn resume(node: &Arc<Node>) {
    let node = node.clone();
    tokio::task::spawn_blocking(move || {
        let result = (|| -> io::Result<()> {
            for folder in node.world.book_directories("received-controls")? {
                for path in items::entries(&folder, Some("json"))? {
                    match items::get::<Entry>(&path) {
                        Ok(Some(entry)) => wake(&node, entry.command.owner()),
                        Ok(None) => {}
                        Err(error) => crate::obligations::retry_record(&node, &path, &error.to_string())?,
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            node.log.line(format_args!("recovering control mail: {error}"));
        }
    });
}
async fn local<T: Send + 'static>(work: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    tokio::task::spawn_blocking(work).await.map_err(io::Error::other)?
}
async fn save(path: &Path, entry: &Entry) -> io::Result<()> {
    let (path, entry) = (path.to_path_buf(), entry.clone());
    local(move || items::keep(&path, &entry)).await
}
async fn retry(node: &Arc<Node>, path: &Path, reason: impl ToString) -> io::Result<()> {
    let (node, path, reason) = (node.clone(), path.to_path_buf(), reason.to_string());
    local(move || crate::obligations::retry_record(&node, &path, &reason)).await
}
async fn finish(node: &Arc<Node>, path: &Path, outcome: Outcome, reason: String) -> io::Result<()> {
    let (node, path) = (node.clone(), path.to_path_buf());
    local(move || {
        let bytes = items::read(&path)?;
        crate::obligations::finish_record(&node, &path, &bytes, outcome, &reason).map(|_| ())
    })
    .await
}
async fn drain(node: Arc<Node>, owner: Address, wake: Arc<Notify>) {
    // Mail is kept as it comes; what it asks is done once the node has
    // recovered what it was doing, which a control may meet.
    node.ready().await;
    let mut again = crate::again::Again::default();
    let (mut after, mut key) = (Vec::new(), String::new());
    let mut through = Vec::new();
    loop {
        let notified = wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if after.is_empty()
            && let Some(worker) = node.control_letters.workers.lock().expect("control workers").get_mut(&owner)
        {
            worker.dirty = false;
        }
        if after.is_empty() {
            let folder = directory(&node, &owner);
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
        let (folder, cursor, last) = (directory(&node, &owner), after.clone(), key.clone());
        let loaded = local(move || {
            items::queued(&folder, &cursor, &last, &upper).map(|rows| {
                rows.into_iter()
                    .map(|row| {
                        let parsed = serde_json::from_slice::<Entry>(&row.bytes).map_err(io::Error::other);
                        (row.order, row.path, parsed)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .await;
        let mut more = false;
        match loaded {
            Ok(rows) => {
                more = !rows.is_empty();
                for (order, path, entry) in rows {
                    after = order;
                    key = path.file_name().and_then(|name| name.to_str()).expect("control identity").to_string();
                    let result = match entry {
                        Ok(entry) => process(&node, &path, entry).await,
                        Err(error) => Err(error),
                    };
                    if let Err(error) = result
                        && let Err(error) = retry(&node, &path, error).await
                    {
                        node.log.line(format_args!("keeping control mail retry: {error}"));
                    }
                }
            }
            Err(error) => node.log.line(format_args!("processing control mail for {owner}: {error}")),
        }
        if more {
            tokio::task::yield_now().await;
            continue;
        }
        after.clear();
        key.clear();
        let folder = directory(&node, &owner);
        let empty = local(move || items::page(&folder, Some("json"), "", 1).map(|rows| rows.is_empty()))
            .await
            .is_ok_and(|empty| empty);
        let dirty = {
            let mut workers = node.control_letters.workers.lock().expect("control workers");
            let dirty = workers.get(&owner).is_some_and(|worker| worker.dirty);
            if empty && !dirty {
                workers.remove(&owner);
                return;
            }
            dirty
        };
        if dirty {
            continue;
        }
        tokio::select! {()=tokio::time::sleep(again.next())=>{},()=&mut notified=>{}}
    }
}

async fn process(node: &Arc<Node>, path: &Path, mut entry: Entry) -> io::Result<()> {
    if entry.result.is_none() {
        // A control is applied once at a time, to its end: a pass that stops
        // waiting for it leaves it going, and the next waits for that one.
        // It is applied again only after a stop between its application and
        // its result being kept, which each command bears: a cancellation is
        // kept again, a stopped job is stopped again, a receipt is recorded
        // once.
        let mut applied = applying(node, path, &entry);
        let waited = tokio::time::timeout(Duration::from_secs(120), applied.wait_for(Option::is_some))
            .await
            .map(|ended| ended.map(|result| result.clone().expect("an application that ended")));
        let result = match waited {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Failure::new("the control's application ended without a result")),
            Err(_) => {
                retry(node, path, "the local control has not completed within 120 seconds").await?;
                return Ok(());
            }
        };
        node.control_letters.applying.lock().expect("never poisoned").remove(path);
        match result {
            Ok(()) => entry.result = Some(Ok(())),
            Err(error) if crate::obligations::permanent(&error) => entry.result = Some(Err(error)),
            Err(error) => {
                retry(node, path, &error.message).await?;
                return Ok(());
            }
        }
        save(path, &entry).await?;
    }
    let result = entry.result.clone().expect("applied control");
    if entry.posted.is_none() {
        let acting = fastctx_types::access::Acting::one(entry.scope.hub.clone(), entry.scope.account.clone());
        let id = crate::speech::fixed_id("control-result", &[entry.request.as_str()]);
        let destination = fastctx_types::access::Written {
            name: entry.from.clone(),
            hub: node.link_of(&entry.scope.hub).map(|link| link.hub_name()),
        };
        let (checking, account, recipient) = (node.clone(), acting.clone(), destination.clone());
        if let Some(reason) = local(move || crate::obligations::impossible(&checking, &account, &recipient)).await? {
            let reason = match &result {
                Ok(()) => format!("control applied; its result cannot be returned: {reason}"),
                Err(error) => format!("{}; the refusal cannot be returned: {reason}", error.message),
            };
            return finish(node, path, Outcome::Impossible, reason).await;
        }
        let (posting, request, outcome) = (node.clone(), entry.request.clone(), result.clone());
        let posted = local(move || {
            crate::letters::post(
                &posting,
                &acting,
                &destination,
                id,
                fastctx_types::remote::Letter::ControlResult { request, result: outcome },
            )
        })
        .await;
        match posted {
            Ok(posted) => {
                entry.posted = Some(posted);
                save(path, &entry).await?;
            }
            Err(error) => {
                retry(node, path, error).await?;
                return Ok(());
            }
        }
    }
    let (checking, posted) = (node.clone(), entry.posted.clone().expect("posted result"));
    let (outcome, reason) = match local(move || posted.state(&checking)).await? {
        fastctx_net::link::Posting::Pending => return Ok(()),
        fastctx_net::link::Posting::Replaced => {
            entry.posted = None;
            save(path, &entry).await?;
            return Ok(());
        }
        fastctx_net::link::Posting::Held => match result {
            Ok(()) => (Outcome::Delivered, "control applied and its result is held by the hub".into()),
            Err(error) => (Outcome::Impossible, error.message),
        },
        fastctx_net::link::Posting::Refused(reason) => (Outcome::Impossible, reason),
    };
    finish(node, path, outcome, reason).await
}

/// The application of the control kept at `path`: the one under way, or a
/// new one in a task of its own.
fn applying(node: &Arc<Node>, path: &Path, entry: &Entry) -> Applied {
    let mut applying = node.control_letters.applying.lock().expect("never poisoned");
    if let Some(applied) = applying.get(path) {
        return applied.clone();
    }
    let (done, applied) = tokio::sync::watch::channel(None);
    applying.insert(path.to_path_buf(), applied.clone());
    let (node, entry) = (node.clone(), entry.clone());
    fastctx_platform::task::spawn(async move {
        let _ = done.send(Some(apply(&node, &entry).await));
    });
    applied
}

async fn apply(node: &Arc<Node>, entry: &Entry) -> Result<(), Failure> {
    match &entry.command {
        ControlLetter::CompletionReceipt { owner, delivery, origin, through_exit } => {
            let (node, owner, delivery, origin, through_exit, scope) =
                (node.clone(), owner.clone(), delivery.clone(), origin.clone(), *through_exit, entry.scope.clone());
            local(move || {
                crate::retention::received_from(&node, &owner, &delivery, origin.as_ref(), through_exit, Some(&scope))
            })
            .await
            .map_err(|error| Failure::new(error.to_string()))
        }
        ControlLetter::StopRun { owner, dispatch } => {
            crate::run_stops::serve(node, owner, dispatch, &entry.scope).await
        }
        ControlLetter::StopOn { origin, job } => {
            crate::on_stop::serve(node, origin.clone(), job.clone()).await.map(|_| ())
        }
    }
}

/// A deterministic control identity permits replay after a crash between
/// local posting and remembering that posting's transport reference.
pub(crate) fn id(path: &Path, bytes: &[u8]) -> NoticeId {
    crate::speech::fixed_id("control", &[&path.to_string_lossy(), &fastctx_hands::files::content_hash(bytes)])
}
