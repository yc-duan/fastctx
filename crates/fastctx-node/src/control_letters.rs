//! Admitted control mail is durable before network acknowledgement. Work is
//! serialized per owner, including guest owners with no host on this node.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_net::link::Caller;
use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
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
    workers: Mutex<HashMap<Address, Arc<Notify>>>,
    pub(crate) changed: Notify,
}

#[derive(Serialize, Deserialize)]
struct Entry {
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
    ensure_private_dir(&directory)?;
    let path = directory.join(format!("{}.json", identity.id));
    let publishing = node.control_letters.workers.lock().expect("never poisoned");
    if !path.with_extension("done").try_exists()? && !path.try_exists()? {
        let sequence = directory.join("sequence");
        let order = match std::fs::read_to_string(&sequence) {
            Ok(text) => text.parse::<u64>().map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        let next = order.checked_add(1).ok_or_else(|| io::Error::other("control letter sequence exhausted"))?;
        publish(&sequence, next.to_string().as_bytes(), Permissions::Private)?;
        let bytes = serde_json::to_vec(&Entry {
            scope,
            command,
            order,
            from: caller.machine.clone(),
            request: identity.original_id().parse().map_err(io::Error::other)?,
            result: refused.map(Err),
            posted: None,
        })
        .map_err(io::Error::other)?;
        publish_new(&path, &bytes, Permissions::Private)?;
    }
    drop(publishing);
    wake(node, &owner);
    Ok(())
}

fn wake(node: &Arc<Node>, owner: &Address) {
    let mut workers = node.control_letters.workers.lock().expect("never poisoned");
    if let Some(wake) = workers.get(owner) {
        wake.notify_one();
        return;
    }
    let wake = Arc::new(Notify::new());
    workers.insert(owner.clone(), wake.clone());
    let (node, owner) = (node.clone(), owner.clone());
    tokio::spawn(async move { drain(node, owner, wake).await });
}

fn paths(directory: &Path) -> io::Result<Vec<PathBuf>> {
    Ok(crate::retention::entries(directory)?
        .into_iter()
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect())
}

pub(crate) fn resume(node: &Arc<Node>) {
    let result = (|| -> io::Result<()> {
        for directory in node.world.book_directories("received-controls")? {
            for path in paths(&directory)? {
                match fastctx_install::renewal::read_json::<Entry>(&path) {
                    Ok(Some(entry)) => wake(node, entry.command.owner()),
                    Ok(None) => {}
                    Err(error) => {
                        crate::obligations::retry(node, &path, &error.to_string())?;
                    }
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        node.log.line(format_args!("recovering control mail: {error}"));
    }
}

async fn drain(node: Arc<Node>, owner: Address, wake: Arc<Notify>) {
    let mut again = crate::again::Again::default();
    loop {
        let notified = wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let result = async {
            let mut entries = Vec::new();
            for path in paths(&directory(&node, &owner))? {
                let read = std::fs::read(&path).and_then(|bytes| {
                    serde_json::from_slice::<Entry>(&bytes).map(|entry| (bytes, entry)).map_err(io::Error::other)
                });
                match read {
                    Ok((bytes, entry)) => entries.push((path, bytes, entry)),
                    Err(error) => {
                        crate::obligations::retry(&node, &path, &error.to_string())?;
                    }
                }
            }
            entries.sort_by_key(|(_, _, entry)| entry.order);
            for (path, _, mut entry) in entries {
                if entry.result.is_none() {
                    match tokio::time::timeout(Duration::from_secs(120), apply(&node, &entry)).await {
                        Ok(Ok(())) => entry.result = Some(Ok(())),
                        Ok(Err(error)) if crate::obligations::permanent(&error) => entry.result = Some(Err(error)),
                        Ok(Err(error)) => {
                            crate::obligations::retry(&node, &path, &error.message)?;
                            continue;
                        }
                        Err(_) => {
                            crate::obligations::retry(
                                &node,
                                &path,
                                "the local control has not completed within 120 seconds",
                            )?;
                            continue;
                        }
                    }
                    fastctx_install::renewal::write_json(&path, &entry)?;
                }
                let result = entry.result.clone().expect("applied control");
                if entry.posted.is_none() {
                    let acting =
                        fastctx_types::access::Acting::one(entry.scope.hub.clone(), entry.scope.account.clone());
                    let id = crate::speech::fixed_id("control-result", &[entry.request.as_str()]);
                    let destination = fastctx_types::access::Written {
                        name: entry.from.clone(),
                        hub: node.link_of(&entry.scope.hub).map(|link| link.hub_name()),
                    };
                    if let Some(reason) = crate::obligations::impossible(&node, &acting, &destination)? {
                        let reason = match &result {
                            Ok(()) => format!("control applied; its result cannot be returned: {reason}"),
                            Err(error) => format!("{}; the refusal cannot be returned: {reason}", error.message),
                        };
                        let bytes = std::fs::read(&path)?;
                        crate::obligations::finish(&node, &path, &bytes, Outcome::Impossible, &reason)?;
                        continue;
                    }
                    match crate::letters::post(
                        &node,
                        &acting,
                        &destination,
                        id,
                        fastctx_types::remote::Letter::ControlResult {
                            request: entry.request.clone(),
                            result: result.clone(),
                        },
                    ) {
                        Ok(posted) => {
                            entry.posted = Some(posted);
                            fastctx_install::renewal::write_json(&path, &entry)?;
                        }
                        Err(error) => {
                            crate::obligations::retry(&node, &path, &error.to_string())?;
                            continue;
                        }
                    }
                }
                let (outcome, reason) = match entry.posted.as_ref().expect("posted result").state(&node)? {
                    fastctx_net::link::Posting::Pending => continue,
                    fastctx_net::link::Posting::Replaced => {
                        entry.posted = None;
                        fastctx_install::renewal::write_json(&path, &entry)?;
                        continue;
                    }
                    fastctx_net::link::Posting::Held => match result {
                        Ok(()) => (Outcome::Delivered, "control applied and its result is held by the hub".to_string()),
                        Err(error) => (Outcome::Impossible, error.message),
                    },
                    fastctx_net::link::Posting::Refused(reason) => (Outcome::Impossible, reason),
                };
                let bytes = std::fs::read(&path)?;
                crate::obligations::finish(&node, &path, &bytes, outcome, &reason)?;
            }
            Ok::<(), io::Error>(())
        }
        .await;
        if let Err(error) = result {
            node.log.line(format_args!("processing control mail for {owner}: {error}"));
        }
        {
            let mut workers = node.control_letters.workers.lock().expect("never poisoned");
            if paths(&directory(&node, &owner)).is_ok_and(|paths| paths.is_empty()) {
                workers.remove(&owner);
                return;
            }
        }
        tokio::select! {
            () = tokio::time::sleep(again.next()) => {}
            () = &mut notified => {}
        }
    }
}

async fn apply(node: &Arc<Node>, entry: &Entry) -> Result<(), Failure> {
    match &entry.command {
        ControlLetter::CompletionReceipt { owner, delivery, origin, through_exit } => {
            crate::retention::received_from(node, owner, delivery, origin.as_ref(), *through_exit, Some(&entry.scope))
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
