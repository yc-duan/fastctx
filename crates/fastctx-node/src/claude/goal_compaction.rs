//! Retained goal compaction requests survive the node that submitted them.

use crate::Node;
use fastctx_hosts::claude::control::{self as files, Host};
use fastctx_types::ipc::Failure;
use fastctx_types::session::control::Controlled;
use fastctx_types::{ProcessIdentity, ResidentId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Serialize, Deserialize)]
struct Ticket {
    key: String,
    host: Host,
    controller: ProcessIdentity,
    #[serde(default)]
    evidence: super::control_outcome::Evidence,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    host: Host,
    reply: Value,
}

fn record(node: &Node, resident: &ResidentId, key: &str, suffix: &str) -> Result<PathBuf, Failure> {
    if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
        return Err(Failure::new("invalid goal compaction request ID"));
    }
    Ok(node.world.conversation(resident).books().join("goal-compactions").join(format!("{key}.{suffix}.json")))
}

/// Called under the session's report gate, before orphan-reply collection.
/// The receipt stays with the resident even after the bridge files are collected.
pub(super) fn capture(node: &Node, resident: &ResidentId, directory: &Path, key: &str) -> std::io::Result<()> {
    let Some(ticket) = files::read::<Ticket>(&directory.join("retained").join(format!("{key}.json")))? else {
        return Ok(());
    };
    if ticket.key != key
        || directory.parent().and_then(Path::file_name).and_then(|name| name.to_str()) != Some(&ticket.host.session)
    {
        return Err(std::io::Error::other("retained compaction belongs to another request or session"));
    }
    let path = record(node, resident, key, "reply").map_err(|error| std::io::Error::other(error.message))?;
    let reply = files::read_message(&directory.join("replies").join(format!("{key}.json")))?;
    files::write(&path, &Receipt { host: ticket.host, reply })
}

/// Publication owns the conversation's action gate; waiting for the receipt
/// releases it so owner controls remain available. A published request is
/// observed again, never reissued, even when its submitting node has exited.
pub(crate) async fn run(
    node: &Arc<Node>,
    resident: &ResidentId,
    key: &str,
    action: fastctx_platform::doing::MutexGuard<'_, ()>,
) -> Result<Controlled, Failure> {
    let ticket_path = record(node, resident, key, "control")?;
    let receipt_path = record(node, resident, key, "reply")?;
    let mut ticket = files::read::<Ticket>(&ticket_path).map_err(failure)?;
    let mut published = false;
    if let Some(prior) = &ticket {
        if prior.key != key {
            return Err(Failure::new("the retained compaction has a different request ID"));
        }
        let gate = super::reading(node, &prior.host.session);
        let _reading = gate.lock().await;
        if let Some(receipt) = files::read::<Receipt>(&receipt_path).map_err(failure)? {
            return result(prior, receipt);
        }
        let directory = files::directory(node.world.path(), &prior.host.session);
        published = directory.join("inbox").join(format!("{key}.json")).try_exists().map_err(failure)?;
    }
    if !published {
        if !crate::goals::engine::requests(node, resident, key) {
            return Ok(Controlled::Refused {
                said: "The goal no longer requests this compaction.".into(),
                current_model: None,
            });
        }
        if crate::control_queue::busy(node, resident)? {
            return Err(Failure::not_run("goal compaction is waiting for the current turn"));
        }
        {
            let management = node.residents.management(resident)?;
            let _reading = management.read().await;
            let input = key.parse().map_err(failure)?;
            crate::lifecycle::retry::prepare_work(node, resident, &input).await?;
        }
        if !crate::goals::engine::requests(node, resident, key) {
            return Ok(Controlled::Refused {
                said: "The goal no longer requests this compaction.".into(),
                current_model: None,
            });
        }
        let (directory, host) = super::control::live_host(node, resident)?;
        let evidence = super::control_outcome::Evidence::capture(&directory, &json!({"do":"goal_compact"}))
            .map_err(|error| Failure::not_run(format!("preparing Claude compaction evidence: {error}")))?;
        let prepared = Ticket {
            key: key.into(),
            host,
            controller: fastctx_platform::current_identity().map_err(failure)?,
            evidence,
        };
        files::write(&ticket_path, &prepared).map_err(failure)?;
        files::write(&directory.join("retained").join(format!("{key}.json")), &prepared).map_err(failure)?;
        files::write_message(
            &directory.join("inbox").join(format!("{key}.json")),
            &json!({
                "instance":prepared.host.instance, "controller":prepared.controller, "control":{"do":"goal_compact"},
                "handoff_contract": if prepared.host.handoff_contract == Some(1) { 1 } else { 0 },
            }),
        )
        .map_err(failure)?;
        ticket = Some(prepared);
    }
    drop(action);
    let ticket = ticket.expect("a prepared or published request");
    let directory = files::directory(node.world.path(), &ticket.host.session);
    let began = Instant::now();
    let handoff = ticket.host.handoff_contract == Some(1);
    let mut evidence = ticket.evidence.clone();
    loop {
        if handoff
            && files::read::<files::Claim>(&directory.join("claims").join(format!("{key}.json")))
                .map_err(uncertain)?
                .filter(|claim| !claim.released)
                .is_none()
            && crate::goals::engine::requests(node, resident, key)
            && !super::control::running(node, resident).map_err(uncertain)?
        {
            let gate = crate::control_queue::gate(node, resident);
            let _action = gate.lock().await;
            super::resume::ready(node, resident).await.map_err(|error| {
                super::control_outcome::unknown(&format!(
                    "reconnecting the submitted goal compaction: {}",
                    error.message
                ))
            })?;
        }
        {
            let gate = super::reading(node, &ticket.host.session);
            let _reading = gate.lock().await;
            if directory.join("replies").join(format!("{key}.done")).try_exists().map_err(uncertain)? {
                capture(node, resident, &directory, key).map_err(uncertain)?;
                super::control::collect_invocation(&directory, key).map_err(uncertain)?;
            }
            if let Some(receipt) = files::read::<Receipt>(&receipt_path).map_err(uncertain)? {
                return result(&ticket, receipt);
            }
            let claimed = files::read::<files::Claim>(&directory.join("claims").join(format!("{key}.json")))
                .map_err(uncertain)?
                .filter(|claim| !claim.released);
            if let Some(claim) = &claimed {
                evidence.executing(claim).map_err(|error| {
                    super::control_outcome::unknown(&format!("reading goal compaction claim: {error}"))
                })?;
            }
            let executing = claimed.as_ref().map(|claim| &claim.host).unwrap_or(&ticket.host);
            if (!handoff || claimed.is_some()) && !files::active(&directory, executing).map_err(uncertain)? {
                if evidence.completed().map_err(|error| {
                    super::control_outcome::unknown(&format!("checking goal compaction evidence: {error}"))
                })? {
                    let receipt =
                        Receipt { host: ticket.host.clone(), reply: json!({"ok":super::control_outcome::done()}) };
                    files::write(&receipt_path, &receipt).map_err(uncertain)?;
                    return result(&ticket, receipt);
                }
                if !evidence.may_finish().map_err(uncertain)? {
                    return Err(super::control_outcome::unknown(
                        "the Claude activation ended without confirming its goal compaction",
                    ));
                }
            }
        }
        if !crate::goals::engine::requests(node, resident, key) {
            return Ok(Controlled::Refused {
                said: "The goal no longer requests this compaction.".into(),
                current_model: None,
            });
        }
        if began.elapsed() >= crate::control_queue::COMPACTING {
            return Err(super::control_outcome::unknown(
                "Claude did not confirm goal compaction within thirty minutes",
            ));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Releases bridge files only after the caller has persisted the final outcome.
/// An already running host operation may finish later; its reply is then ordinary
/// orphan data and cannot cause this request to run again.
pub(crate) async fn release(node: &Node, resident: &ResidentId, key: &str) -> Result<(), Failure> {
    let path = record(node, resident, key, "control")?;
    if let Some(ticket) = files::read::<Ticket>(&path).map_err(failure)? {
        let gate = super::reading(node, &ticket.host.session);
        let _reading = gate.lock().await;
        let directory = files::directory(node.world.path(), &ticket.host.session);
        super::control::collect_invocation(&directory, key).map_err(failure)?;
    }
    Ok(())
}

fn result(ticket: &Ticket, receipt: Receipt) -> Result<Controlled, Failure> {
    if receipt.host != ticket.host {
        return Err(super::control_outcome::unknown("the goal compaction receipt belongs to another activation"));
    }
    let value = super::control_outcome::reply(receipt.reply)?;
    serde_json::from_value(value)
        .map_err(|error| super::control_outcome::unknown(&format!("reading the goal compaction result: {error}")))
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

fn uncertain(error: impl std::fmt::Display) -> Failure {
    super::control_outcome::unknown(&format!("observing submitted goal compaction: {error}"))
}
