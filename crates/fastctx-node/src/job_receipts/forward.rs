use super::Sending;
use crate::{
    Node,
    obligations::{self, Outcome},
    records::items,
};
use fastctx_platform::task;
use fastctx_types::{access::Written, remote::RemoteReply};
use std::{io, path::Path, sync::Arc};

/// Forwarding receipts is a pass (`crate::passes`) of its own: artifact
/// maintenance neither paces it nor waits behind a remote receipt. It is
/// made when a receipt is kept or a way to another machine opens, and once
/// an hour besides.
const FORWARDING: crate::passes::Pass = crate::passes::Pass {
    name: "receipts/forward",
    within: std::time::Duration::from_secs(30 * 60),
    rests: Some(std::time::Duration::from_secs(3600)),
};

/// What the last pass that failed said, so the log does not repeat it.
static SAID: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub(crate) fn start(node: &std::sync::Arc<Node>) {
    use crate::passes::Left;
    crate::passes::register(node, FORWARDING, |node: Arc<Node>| async move {
        match forward(&node).await {
            Ok(pending) => {
                *SAID.lock().expect("never poisoned") = None;
                if pending { Left::Work } else { Left::Nothing }
            }
            Err(error) => {
                let reason = error.to_string();
                let mut said = SAID.lock().expect("never poisoned");
                if said.as_ref() != Some(&reason) {
                    node.log.line(format_args!("forwarding job receipts: {reason}"));
                    *said = Some(reason);
                }
                Left::Work
            }
        }
    });
    crate::passes::follow(node, FORWARDING.name, node.ways.subscribe(), std::time::Duration::ZERO);
}

/// A receipt was kept, or one could not be finished: asks for a pass.
pub(crate) fn changed(node: &Node) {
    crate::passes::ask(node, FORWARDING.name);
}

async fn retry(node: &Arc<Node>, path: &Path, reason: String) -> io::Result<()> {
    let (node, path) = (node.clone(), path.to_path_buf());
    task::spawn_blocking(move || obligations::retry_record(&node, &path, &reason)).await.map_err(io::Error::other)?
}

async fn finish(node: &Arc<Node>, path: &Path, bytes: Vec<u8>, outcome: Outcome, reason: String) -> io::Result<bool> {
    let (node, path) = (node.clone(), path.to_path_buf());
    task::spawn_blocking(move || {
        let done = obligations::finish_record(&node, &path, &bytes, outcome, &reason)?;
        if !done {
            changed(&node);
        }
        Ok(done)
    })
    .await
    .map_err(io::Error::other)?
}

async fn forward(node: &Arc<Node>) -> io::Result<bool> {
    let world = node.world.clone();
    let directories =
        task::spawn_blocking(move || world.book_directories("job-deliveries")).await.map_err(io::Error::other)??;
    let mut pending = false;
    for directory in directories {
        let mut after = String::new();
        loop {
            let (folder, cursor) = (directory.clone(), after.clone());
            let paths = task::spawn_blocking(move || items::page(&folder, Some("json"), &cursor, 128))
                .await
                .map_err(io::Error::other)?;
            let paths = match paths {
                Ok(paths) => paths,
                Err(error) => {
                    pending = true;
                    retry(node, &directory.join("directory"), error.to_string()).await?;
                    break;
                }
            };
            let Some(last) = paths.last() else { break };
            after = last.file_name().and_then(|name| name.to_str()).expect("receipt identity").to_string();
            for path in paths {
                if path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')) {
                    continue;
                }
                match forward_one(node, &path).await {
                    Ok(done) => pending |= !done,
                    Err(error) => {
                        pending = true;
                        retry(node, &path, error.to_string()).await?;
                    }
                }
            }
        }
    }
    Ok(pending)
}

enum Prepared {
    Done(bool),
    Send(Vec<u8>, Box<Sending>, Written),
}

fn prepare(node: &Arc<Node>, path: &Path) -> io::Result<Prepared> {
    let bytes = match items::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Prepared::Done(true)),
        Err(error) => return Err(error),
    };
    let sending: Sending = serde_json::from_slice(&bytes)?;
    let destination = Written { name: sending.machine.clone(), hub: sending.hub.clone() };
    if obligations::finished_record(path, &bytes)? {
        return obligations::finish_record(
            node,
            path,
            &bytes,
            Outcome::Superseded,
            "duplicate of an already settled receipt",
        )
        .map(Prepared::Done);
    }
    let impossible = if sending.delivery.is_some() {
        obligations::control_impossible(node, &sending.acting, &destination)?
    } else {
        obligations::impossible(node, &sending.acting, &destination)?
    };
    if let Some(reason) = impossible {
        return obligations::finish_record(node, path, &bytes, Outcome::Impossible, &reason).map(Prepared::Done);
    }
    Ok(Prepared::Send(bytes, Box::new(sending), destination))
}

async fn forward_one(node: &Arc<Node>, path: &Path) -> io::Result<bool> {
    let (working, reading) = (node.clone(), path.to_path_buf());
    let prepared = task::spawn_blocking(move || prepare(&working, &reading)).await.map_err(io::Error::other)??;
    let (bytes, sending, destination) = match prepared {
        Prepared::Done(done) => return Ok(done),
        Prepared::Send(bytes, sending, destination) => (bytes, *sending, destination),
    };
    let receipts = {
        let (asking, destination) = (node.clone(), destination.clone());
        task::blocking(move || crate::facts::completion_receipts(&asking, &destination)).await
    };
    if !receipts {
        let whole = sending
            .delivery
            .as_ref()
            .filter(|delivery| delivery.shown_whole)
            .filter(|_| !sending.through_exit && sending.origin.is_none());
        if let Some(delivery) = whole {
            let call = fastctx_types::remote::RemoteCall::Forget {
                resident: sending.owner.resident.clone(),
                town: sending.town.clone(),
                job: delivery.job.clone(),
                delivery: None,
                origin: None,
                through_exit: false,
            };
            match node.call(&sending.acting, &destination, fastctx_types::access::Purpose::Hands, call).await {
                Ok(Ok(RemoteReply::Set)) => {}
                other => {
                    retry(node, path, format!("legacy whole-answer cleanup: {other:?}")).await?;
                    return Ok(false);
                }
            }
        }
        return finish(
            node,
            path,
            bytes,
            Outcome::Superseded,
            "the legacy peer does not use separate completion receipts".into(),
        )
        .await;
    }
    let result = if let Some(delivery) = sending.delivery {
        let command = fastctx_types::delivery_control::ControlLetter::CompletionReceipt {
            owner: sending.owner,
            delivery,
            origin: sending.origin,
            through_exit: sending.through_exit,
        };
        if crate::facts::control_letters(node, &destination) {
            let (working, posting, body) = (node.clone(), path.to_path_buf(), bytes.clone());
            let result = task::spawn_blocking(move || {
                crate::control_posts::send(&working, &posting, &body, &sending.acting, &destination, command)
            })
            .await
            .map_err(io::Error::other)??;
            let Some(result) = result else {
                return Ok(false);
            };
            result
        } else {
            // An old peer has no receipt-letter format. Keep its existing
            // call durable here without a capability preflight.
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                node.call(&sending.acting, &destination, fastctx_types::access::Purpose::Hands, command.call()),
            )
            .await
            {
                Ok(Ok(Ok(RemoteReply::Set))) => Ok(()),
                Ok(Ok(Err(error))) => Err(error),
                Ok(other) => Err(fastctx_types::ipc::Failure::new(format!("legacy receipt is unanswered: {other:?}"))),
                Err(_) => Err(fastctx_types::ipc::Failure::new("legacy receipt did not answer within 10 seconds")),
            }
        }
    } else if let Some(consumption) = sending.consumption {
        crate::speech::tell(
            node,
            &sending.acting,
            &sending.owner,
            consumption.id,
            consumption.notice,
            crate::speech::Owed::Working,
        )
        .await
        .map(|_| ())
    } else {
        return Err(io::Error::other("invalid completion receipt phase"));
    };
    let (outcome, reason) = match result {
        Ok(()) => (Outcome::Delivered, "receipt delivered".to_string()),
        Err(error) if obligations::permanent(&error) => (Outcome::Impossible, error.message),
        Err(error) => {
            retry(node, path, error.message).await?;
            return Ok(false);
        }
    };
    finish(node, path, bytes, outcome, reason).await
}
