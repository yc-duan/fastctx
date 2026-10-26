//! Foreground completion debts wait for an actual host receipt, or pass
//! custody to the hub. Local outbox publication alone never pays this debt.

use std::io;
use std::path::Path;
use std::time::Duration;

use fastctx_net::link::Posting;
use fastctx_types::access::Acting;
use fastctx_types::remote::Letter;
use fastctx_types::residents::Notice;
use fastctx_types::{Address, NoticeId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Default, Serialize, Deserialize)]
struct Waiting {
    failures: u32,
    due: Option<Timestamp>,
}

pub(crate) fn ready(node: &Node, path: &Path, owner: &Address) -> io::Result<bool> {
    match std::fs::read(path.with_extension("owed"))?.as_slice() {
        b"" => return Ok(true),
        b"answer" => {}
        _ => return Err(io::Error::other("the job's completion debt has an unknown form")),
    }
    let online = node.is_me(&owner.machine)
        || node.locate(&(&owner.machine).into()).is_ok_and(|links| {
            links.iter().any(|link| link.view().member(&owner.machine).is_some_and(|member| member.online))
        });
    if !online {
        return Ok(true);
    }
    let marker = path.with_extension("answer-wait");
    let mut waiting = fastctx_install::renewal::read_json::<Waiting>(&marker)?.unwrap_or_default();
    if waiting.due.is_some_and(|due| due > Timestamp::now()) {
        return Ok(false);
    }
    if fastctx_net::again::at_ceiling(waiting.failures) {
        return Ok(true);
    }
    waiting.failures = waiting.failures.saturating_add(1);
    waiting.due =
        Some(Timestamp::now().checked_add(fastctx_net::again::backoff(waiting.failures)).map_err(io::Error::other)?);
    fastctx_install::renewal::write_json(&marker, &waiting)?;
    Ok(false)
}

pub(crate) fn pause(path: &Path) -> io::Result<Option<Duration>> {
    let Some(waiting) = fastctx_install::renewal::read_json::<Waiting>(&path.with_extension("answer-wait"))? else {
        return Ok(None);
    };
    Ok(waiting
        .due
        .map(|due| {
            Duration::from_millis(due.as_millisecond().saturating_sub(Timestamp::now().as_millisecond()).max(0) as u64)
        })
        .filter(|pause| !pause.is_zero()))
}

pub(crate) fn pending(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, what)
}

/// Publishes once locally; future attempts inspect the hub receipt instead
/// of producing a new copy on every backoff tick.
pub(crate) fn post(
    node: &Node,
    path: &Path,
    owner: &Address,
    acting: &Acting,
    id: NoticeId,
    notice: Notice,
) -> io::Result<Result<(), String>> {
    let marker = path.with_extension("posted");
    let posted = match fastctx_install::renewal::read_json::<crate::letters::Posted>(&marker)? {
        Some(posted) => posted,
        None => {
            let posted = crate::letters::post(
                node,
                acting,
                &(&owner.machine).into(),
                id,
                Letter::Notice { to: owner.resident.clone(), notice: Box::new(notice) },
            )?;
            fastctx_install::renewal::write_json(&marker, &posted)?;
            posted
        }
    };
    match posted.state(node)? {
        Posting::Held => Ok(Ok(())),
        Posting::Pending => Err(pending("the hub has not yet acknowledged the completion letter")),
        Posting::Refused(reason) => Ok(Err(reason)),
        Posting::Replaced => {
            std::fs::remove_file(marker)?;
            Err(pending("the membership's outbox changed; the completion will be posted again"))
        }
    }
}

pub(crate) fn paid(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path.with_extension("owed")) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
