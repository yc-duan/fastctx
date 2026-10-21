//! Resuming a historical conversation for a control, without synthetic input.

use crate::Node;
use fastctx_hosts::claude::control as files;
use fastctx_types::{ResidentId, ipc::Failure};
use std::sync::Arc;
use std::time::Duration;

/// The caller holds the conversation action gate, also used by word delivery.
pub(super) async fn ready(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    if super::control::running(node, id)? {
        return isolated(node, id, false);
    }
    let naming = tokio::time::Instant::now() + Duration::from_secs(120);
    let about = loop {
        node.residents.active(id)?;
        let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new("unknown conversation"))?;
        if about.session.is_some() {
            break about;
        }
        if tokio::time::Instant::now() >= naming {
            return Err(Failure::new(
                "Claude did not identify the conversation within 120 seconds; the command was not sent",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    crate::homes::available(node, &about)?;
    let gate = node.residents.management(id)?;
    let _reading = crate::gate_waits::shared(node, id, &gate).await?;
    let session =
        about.session.as_deref().ok_or_else(|| Failure::new("Claude has not identified this conversation yet"))?;

    if !super::alive(node, about.home.as_deref(), session).await? {
        crate::profile::session::claude(node, id, false)?;
        super::management::wake(node, about.home.as_deref(), session).await?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        node.residents.active(id)?;
        if super::control::running(node, id)? {
            return isolated(node, id, true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure::new(
                "Claude's conversation did not connect its current control plugin within 120 seconds; the command was not sent",
            ));
        }
        tokio::select! {
            () = node.claude.changed.notified() => {},
            () = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}

fn isolated(node: &Node, id: &ResidentId, resumed: bool) -> Result<(), Failure> {
    if crate::assists::host::kept(node, id)? != Some(true) {
        return Ok(());
    }
    let session = node
        .residents
        .about(id)
        .and_then(|(about, _)| about.session)
        .ok_or_else(|| Failure::new("Claude has not identified this conversation"))?;
    let host =
        files::host(&files::directory(node.world.path(), &session)).map_err(|error| Failure::new(error.to_string()))?;
    let reported = host.and_then(|host| host.assisted);
    // An already-running older mod predates this report; its recorded birth
    // still proves isolation. A fresh resume must confirm its actual settings.
    if reported == Some(false) || (resumed && reported != Some(true)) {
        return Err(Failure::new(
            "Claude's control plugin did not confirm assisted host isolation; the command was not sent",
        ));
    }
    Ok(())
}
