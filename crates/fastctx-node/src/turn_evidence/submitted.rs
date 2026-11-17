//! Explicit between-turn compaction is recorded before waiting for the host.

use crate::{Node, submitted::Family};
use fastctx_types::{ResidentId, ipc::Failure, turn_evidence::Compacted};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

const FAMILY: Family = Family {
    name: "call/compact-between-turns",
    at_once: 8,
    // The host may spend thirty minutes compacting after two minutes to begin.
    within: Duration::from_secs(45 * 60),
    stays: Duration::from_secs(240),
    // Repeating a compaction can begin another host turn and spend model tokens.
    again: false,
};

#[derive(Serialize, Deserialize)]
struct Asked {
    resident: ResidentId,
}

pub(crate) fn start(node: &Arc<Node>) {
    crate::submitted::register(node, FAMILY, |node: Arc<Node>, asked: Asked| async move {
        // The original compactor takes the action gate and checks busy again.
        Ok(super::compact::between_turns(&node, &asked.resident).await)
    });
    node.submission_kinds.registered(&FAMILY);
}

async fn rejection(node: &Arc<Node>, resident: &ResidentId) -> Option<Compacted> {
    if let Err(failure) = node.residents.active(resident) {
        return Some(Compacted::Failed { reason: failure.message });
    }
    match crate::control_queue::busy(node, resident).await {
        Ok(false) => None,
        Ok(true) => Some(Compacted::Refused { reason: "a turn is still running".into() }),
        Err(failure) => Some(Compacted::Failed { reason: failure.message }),
    }
}

pub(crate) async fn compact(node: &Arc<Node>, resident: ResidentId) -> Result<Compacted, Failure> {
    if !node.submission_kinds.has(&FAMILY) {
        return Ok(super::compact::between_turns(node, &resident).await);
    }
    let asked = Asked { resident };
    let mut rejected = None;
    let result = crate::submitted::submit_resolving(node, &FAMILY, &asked, || async {
        if let Some(answer) = rejection(node, &asked.resident).await {
            let reason = match &answer {
                Compacted::Refused { reason } | Compacted::Failed { reason } => reason.clone(),
                Compacted::Done => unreachable!("a rejection is not completion"),
            };
            let failure = Failure::not_run(reason);
            rejected = Some((failure.clone(), answer));
            return Err(failure);
        }
        // The task must survive deletion of the target until it settles its status.
        Ok((format!("compact/{}", asked.resident), ()))
    })
    .await;
    match (result, rejected) {
        // A concurrent operation found by the engine takes precedence over this
        // preflight, including an Accepted response rather than the local refusal.
        (Err(failure), Some((expected, answer))) if failure == expected => Ok(answer),
        (Err(failure), _) if crate::submission_fallback::unrecorded(&FAMILY, &failure) => {
            Ok(super::compact::between_turns(node, &asked.resident).await)
        }
        (result, _) => result,
    }
}
