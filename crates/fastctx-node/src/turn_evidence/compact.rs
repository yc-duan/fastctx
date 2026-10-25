//! A goal's between-turn compaction; accepting a Codex request is not completion.

use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::session::control::{Control, Controlled};
use fastctx_types::turn_evidence::Compacted;
use std::sync::Arc;
use std::time::Instant;

pub(crate) async fn between_turns(node: &Arc<Node>, resident: &ResidentId) -> Compacted {
    let result = run(node, resident, None).await;
    result.unwrap_or_else(|error| Compacted::Failed { reason: error.message })
}

pub(crate) async fn recorded(
    node: &Arc<Node>,
    resident: &ResidentId,
    key: &str,
) -> Result<Compacted, fastctx_types::ipc::Failure> {
    match run(node, resident, Some(key)).await {
        Err(error) if error.code == Some(fastctx_types::ipc::FailureCode::HostUnavailable) => Err(error),
        result => Ok(result.unwrap_or_else(|error| Compacted::Failed { reason: error.message })),
    }
}

async fn run(
    node: &Arc<Node>,
    resident: &ResidentId,
    key: Option<&str>,
) -> Result<Compacted, fastctx_types::ipc::Failure> {
    let gate = crate::control_queue::gate(node, resident);
    let action = gate.lock().await;
    node.residents.active(resident)?;
    let (about, _) =
        node.residents.about(resident).ok_or_else(|| fastctx_types::ipc::Failure::new("unknown conversation"))?;
    if about.host.agent.as_str() == "claude"
        && let Some(key) = key
    {
        return match crate::claude::goal_compaction::run(node, resident, key, action).await? {
            Controlled::Done { .. } => Ok(Compacted::Done),
            Controlled::Refused { said, .. } => Ok(Compacted::Refused { reason: said }),
            result => Ok(Compacted::Failed { reason: format!("compaction did not establish completion: {result:?}") }),
        };
    }
    if let Some(key) = key
        && !crate::goals::engine::requests(node, resident, key)
    {
        return Ok(Compacted::Refused { reason: "The goal no longer requests this compaction.".into() });
    }
    if crate::control_queue::busy(node, resident)? {
        return Ok(Compacted::Refused { reason: "a turn is still running".into() });
    }
    let before = node.residents.last_compaction(resident);
    let mut changes = node.residents.changes();
    let result = if about.host.agent.as_str() == "claude" {
        crate::claude::control::goal_compact(node, resident).await?
    } else {
        crate::control::execute(node, resident, Control::Compact).await?
    };
    drop(action);
    match result {
        Controlled::Refused { said, .. } => return Ok(Compacted::Refused { reason: said }),
        Controlled::Done { .. } if about.host.agent.as_str() == "claude" => return Ok(Compacted::Done),
        Controlled::Done { .. } => {}
        result => {
            return Ok(Compacted::Failed { reason: format!("compaction did not establish completion: {result:?}") });
        }
    }
    let began = Instant::now();
    let mut seen_turn = false;
    loop {
        if node.residents.last_compaction(resident).is_some_and(|at| Some(at) != before) {
            return Ok(Compacted::Done);
        }
        let Some(now) = node.residents.now(resident) else {
            return Ok(Compacted::Failed { reason: "the conversation disappeared during compaction".into() });
        };
        seen_turn |= now.turn.is_some();
        if seen_turn && now.turn.is_none() {
            return Ok(Compacted::Failed {
                reason: now
                    .trouble
                    .map(|trouble| trouble.text)
                    .unwrap_or_else(|| "the compaction turn ended without compacting".into()),
            });
        }
        if began.elapsed() >= crate::control_queue::COMPACTING
            || (!seen_turn && began.elapsed() >= crate::control_queue::COMPACTION_BEGINS)
        {
            return Ok(Compacted::Failed { reason: "the host did not confirm compaction".into() });
        }
        tokio::select! {
            changed = changes.changed() => {
                changed.map_err(|_| fastctx_types::ipc::Failure::new("the node is stopping"))?;
            }
            () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
        }
    }
}
