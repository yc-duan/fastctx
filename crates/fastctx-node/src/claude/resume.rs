//! Resuming a historical conversation for a control, without synthetic input.

use crate::Node;
use fastctx_hosts::claude::control as files;
use fastctx_types::{ResidentId, ipc::Failure};
use std::sync::Arc;
use std::time::Duration;

/// The caller holds the conversation action gate, also used by word delivery.
pub(super) async fn ready(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let gate = node.residents.management(id)?;
    let _reading = crate::gate_waits::shared(node, id, &gate).await?;
    crate::lifecycle::wake::wake(node, id, crate::lifecycle::wake::Source::Control, None)
        .await
        .map(|_| ())
        .map_err(crate::lifecycle::wake::WakeFailure::control)
}

pub(crate) async fn connect(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    if ready_activation(node, id)? {
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
            return Err(Failure::not_run(
                "Claude did not identify the conversation within 120 seconds; the command was not sent",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    crate::homes::available(node, &about)?;
    let session =
        about.session.as_deref().ok_or_else(|| Failure::new("Claude has not identified this conversation yet"))?;

    if !super::alive(node, about.home.as_deref(), session).await? {
        let (current, _) = node.residents.about(id).ok_or_else(crate::authorization::no_subject)?;
        crate::homes::listed(node, current.host.agent.as_str(), current.home.as_deref())?;
        crate::assists::may_turn(node, id, &current)?;
        super::management::wake(node, about.home.as_deref(), session).await?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let mut probe = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        node.residents.active(id)?;
        if ready_activation(node, id)? {
            return isolated(node, id, true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure::not_run(
                "Claude's conversation did not connect its current control plugin within 120 seconds; the command was not sent",
            ));
        }
        // A host can still be exiting at the first liveness check, or end
        // during plugin startup. Recheck until the ready receipt arrives.
        if tokio::time::Instant::now() >= probe {
            if !super::alive(node, about.home.as_deref(), session).await? {
                let (current, _) = node.residents.about(id).ok_or_else(crate::authorization::no_subject)?;
                crate::homes::listed(node, current.host.agent.as_str(), current.home.as_deref())?;
                crate::assists::may_turn(node, id, &current)?;
                super::management::wake(node, about.home.as_deref(), session).await?;
            }
            probe = tokio::time::Instant::now() + Duration::from_secs(1);
        }
        tokio::select! {
            () = node.claude.changed.notified() => {},
            () = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}

fn ready_activation(node: &Node, id: &ResidentId) -> Result<bool, Failure> {
    let Some(session) = node.residents.about(id).and_then(|(about, _)| about.session) else { return Ok(false) };
    let directory = files::directory(node.world.path(), &session);
    let Some(host) = files::host(&directory).map_err(|error| Failure::new(error.to_string()))? else {
        return Ok(false);
    };
    if host.command_contract != Some(files::COMMAND_CONTRACT) {
        return Ok(false);
    }
    // host.done identifies the activation before its asynchronous catalog and
    // control scanner are initialized. Wait for the latter's committed receipt.
    let receipt = if host.handoff_contract == Some(1) { "ready" } else { "catalog" };
    Ok(directory
        .join(format!("{receipt}-{}.done", host.instance))
        .try_exists()
        .map_err(|error| Failure::new(error.to_string()))?
        && files::active(&directory, &host).map_err(|error| Failure::new(error.to_string()))?)
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
        return Err(Failure::not_run(
            "Claude's control plugin did not confirm assisted host isolation; the command was not sent",
        ));
    }
    Ok(())
}
