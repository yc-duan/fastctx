//! A durable attempt is recovered from its host's evidence, never blindly repeated.

use super::failure;
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::goals::runtime::Work;
use fastctx_types::ipc::Failure;
use fastctx_types::turn_evidence::Compacted;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
struct Attempt {
    #[serde(default)]
    retained: bool,
    #[serde(default)]
    codex: Option<Boundary>,
    outcome: Option<Compacted>,
}

#[derive(Serialize, Deserialize)]
struct Boundary {
    session: String,
    turn: String,
}

pub(super) fn started(node: &Node, id: &ResidentId, work: &Work) -> Result<bool, Failure> {
    path(node, id, work).try_exists().map_err(failure)
}

fn path(node: &Node, id: &ResidentId, work: &Work) -> std::path::PathBuf {
    node.world.conversation(id).books().join("goal-compactions").join(format!("{}.json", work.id))
}

pub(super) async fn run(node: &Arc<Node>, id: &ResidentId, work: &Work) -> Result<Compacted, Failure> {
    let path = path(node, id, work);
    let prior = fastctx_hosts::claude::control::read::<Attempt>(&path).map_err(failure)?;
    let fresh = prior.is_none();
    if fresh {
        let action = crate::control_queue::gate(node, id);
        let _action = action.lock().await;
        let management = node.residents.management(id)?;
        let _reading = management.read().await;
        if let Err(error) = crate::lifecycle::retry::prepare_work(node, id, &work.id).await {
            if error.code == Some(fastctx_types::ipc::FailureCode::HostUnavailable) {
                return Err(error);
            }
            return Ok(Compacted::Refused { reason: error.message });
        }
    }
    let mut attempt = match prior {
        Some(attempt) => attempt,
        None => {
            let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new("unknown conversation"))?;
            let codex = if about.host.agent.as_str() == "codex" {
                Some(Boundary {
                    session: about.session.ok_or_else(|| Failure::new("the conversation has no Codex thread"))?,
                    turn: work.turn.clone().ok_or_else(|| Failure::new("goal compaction has no failed turn"))?,
                })
            } else {
                None
            };
            let outcome = if let Some(boundary) = &codex {
                completed(node, id, boundary).await?.then_some(Compacted::Done)
            } else {
                None
            };
            let attempt = Attempt { retained: about.host.agent.as_str() == "claude", codex, outcome };
            fastctx_hosts::claude::control::write(&path, &attempt).map_err(failure)?;
            attempt
        }
    };
    if attempt.outcome.is_none() {
        let outcome = if attempt.retained || fresh {
            crate::turn_evidence::compact::recorded(node, id, work.id.as_str()).await?
        } else if let Some(boundary) = &attempt.codex {
            if completed(node, id, boundary).await? {
                Compacted::Done
            } else {
                Compacted::Failed {
                    reason: "The node restarted during compaction and the host has no completed compaction after the failed turn.".into(),
                }
            }
        } else {
            Compacted::Failed { reason: "The interrupted compaction has no recoverable host evidence.".into() }
        };
        attempt.outcome = Some(outcome);
        fastctx_hosts::claude::control::write(&path, &attempt).map_err(failure)?;
    }
    if attempt.retained {
        crate::claude::goal_compaction::release(node, id, work.id.as_str()).await?;
    }
    Ok(attempt.outcome.expect("compaction settled"))
}

async fn completed(node: &Arc<Node>, id: &ResidentId, boundary: &Boundary) -> Result<bool, Failure> {
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new("unknown conversation"))?;
    if about.session.as_deref() != Some(&boundary.session) {
        return Err(Failure::new("the conversation's Codex thread changed during compaction"));
    }
    let host = node.codex.of(node, &about)?;
    host.compacted_after(&boundary.session, &boundary.turn).await.map_err(Failure::new)
}
