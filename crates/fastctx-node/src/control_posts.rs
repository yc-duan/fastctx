//! Durable outgoing control requests and their authenticated business replies.
//! Hub custody does not itself establish that a requested stop was applied.

pub(crate) mod requests;

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
use crate::records::items;

#[derive(Serialize, Deserialize)]
struct Request {
    machine: MachineName,
    hub: HubId,
    account: AccountName,
    path: PathBuf,
    digest: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Posted {
    id: NoticeId,
    posted: crate::letters::Posted,
    /// When it was posted; an earlier release's marker says so by its
    /// file's time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<jiff::Timestamp>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Reply {
    id: NoticeId,
    result: Result<(), Failure>,
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
    if let Some(reply) = items::get::<Reply>(&path.with_extension("reply"))?
        && reply.id == id
    {
        return Ok(Some(reply.result));
    }
    let marker = path.with_extension("posted");
    let posted = match items::get::<Posted>(&marker)? {
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
            requests::keep(node, &id, &request)?;
            let posted = crate::letters::post(
                node,
                acting,
                destination,
                id.clone(),
                Letter::Control { command: Box::new(command) },
            )?;
            let posted = Posted { id, posted, at: Some(jiff::Timestamp::now()) };
            items::keep(&marker, &posted)?;
            posted
        }
    };
    match posted.posted.state(node)? {
        Posting::Pending => Ok(None),
        Posting::Held => {
            Ok(set_aside(node, &marker, &posted, destination)?.map(|reason| Err(Failure::refused(reason))))
        }
        Posting::Replaced => {
            items::delete(&marker)?;
            Ok(None)
        }
        Posting::Refused(reason) => Ok(Some(Err(Failure::refused(reason)))),
    }
}

/// Why the answer to a request the hub took will never come: since it was
/// posted, the hub set aside what waited for the machine it went to, or for
/// this one, one of them having been away more than 30 days
/// (`world-network.md`, 每一笔账都有终点). Only the view given on the
/// connection up now says so.
fn set_aside(node: &Node, marker: &Path, posted: &Posted, destination: &Written) -> io::Result<Option<String>> {
    let Some(link) = node.link_of(posted.posted.hub()) else { return Ok(None) };
    let Ok(view) = crate::authorization::current_view(&link) else { return Ok(None) };
    let at = match posted.at {
        Some(at) => at,
        None => jiff::Timestamp::try_from(items::modified(marker)?).map_err(io::Error::other)?,
    };
    let since = |member: Option<&fastctx_net::directory::Member>| member.and_then(|member| member.settled) > Some(at);
    Ok(if since(view.member(&destination.name)) {
        Some(format!("{} was away more than 30 days, and what waited for it was set aside", destination.name))
    } else if since(view.me()) {
        Some(
            "this computer was away more than 30 days, and its answer was set aside; whether it was done is not known"
                .into(),
        )
    } else {
        None
    })
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
    let Some(request) = requests::get(node, id)? else {
        return Ok(Err("this node has no such outgoing control request".into()));
    };
    let same = node.link_of(hub).is_some_and(|link| link.view().same(&request.machine, &caller.machine));
    if request.hub != *hub || request.account != caller.account || !same {
        return Ok(Err("the control reply is from another machine or account".into()));
    }
    let still_pending = match items::read(&request.path) {
        Ok(bytes) => fastctx_hands::files::content_hash(&bytes) == request.digest,
        // A stop may be posting while its local pending record is still
        // being published. The registered request already binds this reply.
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => return Err(error),
    };
    if still_pending {
        items::keep(&request.path.with_extension("reply"), &Reply { id: id.clone(), result })?;
        node.control_letters.changed.notify_waiters();
        node.job_files.changed.notify_one();
        node.receipts_changed.notify_one();
    } else {
        // Another request has taken its place, which asks and is answered
        // under its own id: this reply is to one no longer asked.
        node.log.line(format_args!("a control reply to {id}, a request since replaced, is let go"));
    }
    Ok(Ok(()))
}

pub(crate) fn collect(node: &Node, age: std::time::Duration) -> io::Result<()> {
    let mut after = String::new();
    loop {
        let page = requests::page(node, &after)?;
        let Some(last) = page.last() else { return Ok(()) };
        after = last.0.clone();
        for (id, modified, request) in page {
            if !items::exists(&request.path)?
                && jiff::Timestamp::now().as_second().saturating_sub(modified.as_second()) >= age.as_secs() as i64
            {
                for extension in ["reply", "posted"] {
                    match items::delete(&request.path.with_extension(extension)) {
                        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                        _ => {}
                    }
                }
                requests::delete(node, id)?;
            }
        }
    }
}
