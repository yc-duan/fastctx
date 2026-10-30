//! A fork is usable only after its mod and native transcript are ready.

use std::sync::Arc;
use std::time::Duration;

use fastctx_hosts::claude::{control, exchange};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Activity, Trouble};
use fastctx_types::session::{Health, HealthKind};
use fastctx_types::{ProcessIdentity, ResidentId};
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Deserialize, Serialize)]
struct Preparing {
    owner: ProcessIdentity,
    failure: Option<String>,
}

fn path(node: &Node, resident: &ResidentId) -> std::path::PathBuf {
    node.world.conversation(resident).path().join("claude-fork-preparing.json")
}

pub(super) fn begin(node: &Node, resident: &ResidentId) -> Result<(), Failure> {
    control::write(
        &path(node, resident),
        &Preparing { owner: fastctx_platform::current_identity().map_err(failure)?, failure: None },
    )
    .map_err(failure)
}

/// An interrupted prefix change must not admit input into the untrimmed fork.
pub(super) fn pending(node: &Node, resident: &ResidentId) -> Result<Option<String>, Failure> {
    let Some(preparing) = control::read::<Preparing>(&path(node, resident)).map_err(failure)? else { return Ok(None) };
    Ok(Some(match preparing.failure {
        Some(why) => {
            format!("this fork is incomplete: {why}; queued words have not been submitted; fork the source again")
        }
        None if !fastctx_platform::is_alive(&preparing.owner).map_err(failure)? => {
            "the node stopped before finishing this fork; queued words have not been submitted; fork the source again"
                .into()
        }
        None => "waiting for Claude to finish copying the fork's context and history".into(),
    }))
}

pub(super) fn failed(node: &Arc<Node>, resident: &ResidentId, why: &str) -> Result<(), Failure> {
    if let Some(mut preparing) = control::read::<Preparing>(&path(node, resident)).map_err(failure)? {
        preparing.failure = Some(why.into());
        control::write(&path(node, resident), &preparing).map_err(failure)?;
    }
    let text = format!("Claude fork incomplete: {why}; fork the source again");
    node.log.line(&text);
    crate::turns::health(node, resident, None, Health { kind: HealthKind::Error, text: text.clone() })
        .map_err(failure)?;
    node.residents
        .change_now(&node.world, resident, |now| {
            if now.turn.is_none() {
                now.activity = Activity::Idle;
                now.since = jiff::Timestamp::now();
                now.trouble = Some(Trouble { at: now.since, text });
            }
        })
        .map_err(failure)
}

pub(super) async fn finish(node: &Arc<Node>, resident: &ResidentId) -> Result<(), Failure> {
    std::fs::remove_file(path(node, resident)).map_err(failure)?;
    node.residents
        .change_now(&node.world, resident, |now| {
            if now.turn.is_none() {
                now.activity = fastctx_types::residents::Activity::Idle;
            }
        })
        .map_err(failure)?;
    // Release waiting input through ordinary admission once the fork is whole.
    crate::lifecycle::retry::kick(node, resident);
    Ok(())
}

pub(super) fn recover(node: &Arc<Node>, records: &[crate::residents::OnRecord]) {
    for record in records {
        if record.removed.is_some() || record.about.host.agent.as_str() != "claude" {
            continue;
        }
        let result = (|| {
            if let Some(preparing) = control::read::<Preparing>(&path(node, &record.id)).map_err(failure)?
                && preparing.failure.is_none()
                && !fastctx_platform::is_alive(&preparing.owner).map_err(failure)?
            {
                failed(node, &record.id, "the node stopped before confirming the copied context")?;
            }
            Ok::<_, Failure>(())
        })();
        if let Err(error) = result {
            node.log.line(format_args!("recovering Claude fork {}: {}", record.id, error.message));
        }
    }
}

pub(super) async fn wait(node: &Arc<Node>, resident: &ResidentId, saved: bool) -> Result<(), Failure> {
    let deadline = tokio::time::sleep(Duration::from_secs(120));
    tokio::pin!(deadline);
    super::replay(node).await;
    loop {
        let changed = node.claude.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        node.residents.active(resident)?;
        let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("the fork was removed"))?;
        if let Some(session) = about.session {
            let directory = exchange::sessions(node.world.path()).join(&session);
            let host = control::host(&directory.join("control")).map_err(failure)?;
            if registered(node, resident).map_err(failure)?
                && host.as_ref().is_some_and(|host| control::active(&directory.join("control"), host).unwrap_or(false))
                && (!saved || exchange::transcript(&directory).map_err(failure)?.is_some_and(|path| path.is_file()))
            {
                return Ok(());
            }
        }
        tokio::select! {
            () = &mut changed => {},
            () = tokio::time::sleep(fastctx_hosts::claude::MOD_LOOK) => {},
            () = &mut deadline => return Err(Failure::new("Claude's fork did not finish registering its control bridge and saved history within 120 s")),
        }
    }
}

/// Whether the fork's session is paired and its mod registered whole, in a
/// process that is alive.
fn registered(node: &Node, resident: &ResidentId) -> std::io::Result<bool> {
    if !super::paired(node, resident) {
        return Ok(false);
    }
    let Some(session) = node.residents.about(resident).and_then(|(about, _)| about.session) else { return Ok(false) };
    let registration = exchange::registration(&exchange::sessions(node.world.path()).join(session))?;
    match registration.filter(|registration| registration.instance.is_some() && registration.process.is_some()) {
        Some(registration) => super::pairing::present(node, &registration),
        None => Ok(false),
    }
}

fn failure(error: std::io::Error) -> Failure {
    Failure::new(error.to_string())
}
