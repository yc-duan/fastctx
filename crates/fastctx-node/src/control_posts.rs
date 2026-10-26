//! Durable outgoing control requests and their authenticated business replies.
//! Hub custody does not itself establish that a requested stop was applied.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_net::link::{Caller, Posting};
use fastctx_types::access::{Acting, HubId, Written};
use fastctx_types::delivery_control::ControlLetter;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::Letter;
use fastctx_types::{AccountName, MachineName, NoticeId};
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Serialize, Deserialize)]
struct Request {
    machine: MachineName,
    hub: HubId,
    account: AccountName,
    path: PathBuf,
    digest: String,
}

#[derive(Serialize, Deserialize)]
struct Posted {
    id: NoticeId,
    posted: crate::letters::Posted,
}

#[derive(Serialize, Deserialize)]
struct Reply {
    id: NoticeId,
    result: Result<(), Failure>,
}

fn request_path(node: &Node, id: &NoticeId) -> PathBuf {
    node.world.path().join("control-requests").join(format!("{id}.json"))
}

pub(crate) fn send(
    node: &Node,
    path: &Path,
    bytes: &[u8],
    acting: &Acting,
    destination: &Written,
    command: ControlLetter,
) -> io::Result<Option<Result<(), Failure>>> {
    let id = crate::control_letters::id(path, bytes);
    if let Some(reply) = fastctx_install::renewal::read_json::<Reply>(&path.with_extension("reply"))?
        && reply.id == id
    {
        return Ok(Some(reply.result));
    }
    let marker = path.with_extension("posted");
    let posted = match fastctx_install::renewal::read_json::<Posted>(&marker)? {
        Some(posted) if posted.id == id => posted,
        _ => {
            let (link, account) =
                node.mailbox(acting, destination, fastctx_types::access::Purpose::Hands).map_err(io::Error::other)?;
            let request = Request {
                machine: destination.name.clone(),
                hub: link.membership().id.clone(),
                account,
                path: path.to_path_buf(),
                digest: fastctx_hands::files::content_hash(bytes),
            };
            let request_file = request_path(node, &id);
            fastctx_platform::ensure_private_dir(request_file.parent().expect("control requests directory"))?;
            fastctx_install::renewal::write_json(&request_file, &request)?;
            let posted = crate::letters::post(
                node,
                acting,
                destination,
                id.clone(),
                Letter::Control { command: Box::new(command) },
            )?;
            let posted = Posted { id, posted };
            fastctx_install::renewal::write_json(&marker, &posted)?;
            posted
        }
    };
    match posted.posted.state(node)? {
        Posting::Pending | Posting::Held => Ok(None),
        Posting::Replaced => {
            std::fs::remove_file(marker)?;
            Ok(None)
        }
        Posting::Refused(reason) => Ok(Some(Err(Failure::refused(reason)))),
    }
}

/// A reply names a request recorded by this node before posting. Its hub,
/// account and authenticated sender must all match that request.
pub(crate) fn receive(
    node: &Node,
    hub: &HubId,
    caller: &Caller,
    id: &NoticeId,
    result: Result<(), Failure>,
) -> io::Result<Result<(), String>> {
    let Some(request) = fastctx_install::renewal::read_json::<Request>(&request_path(node, id))? else {
        return Ok(Err("this node has no such outgoing control request".into()));
    };
    let same = node.link_of(hub).is_some_and(|link| link.view().same(&request.machine, &caller.machine));
    if request.hub != *hub || request.account != caller.account || !same {
        return Ok(Err("the control reply is from another machine or account".into()));
    }
    let still_pending = match std::fs::read(&request.path) {
        Ok(bytes) => fastctx_hands::files::content_hash(&bytes) == request.digest,
        // A stop may be posting while its local pending record is still
        // being published. The registered request already binds this reply.
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => return Err(error),
    };
    if still_pending {
        fastctx_install::renewal::write_json(&request.path.with_extension("reply"), &Reply { id: id.clone(), result })?;
        node.control_letters.changed.notify_waiters();
        node.disk_changed.notify_one();
        node.receipts_changed.notify_one();
    }
    Ok(Ok(()))
}

pub(crate) fn collect(node: &Node, age: std::time::Duration) -> io::Result<()> {
    for path in crate::retention::entries(&node.world.path().join("control-requests"))? {
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let request = match fastctx_install::renewal::read_json::<Request>(&path) {
            Ok(Some(request)) => request,
            Ok(None) => continue,
            Err(error) => {
                crate::obligations::retry(node, &path, &error.to_string())?;
                continue;
            }
        };
        if !request.path.try_exists()? && path.metadata()?.modified()?.elapsed().is_ok_and(|elapsed| elapsed >= age) {
            for extension in ["reply", "posted"] {
                match std::fs::remove_file(request.path.with_extension(extension)) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
            }
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}
