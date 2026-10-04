//! The node owns the active-call index. A helper's actual process lifetime
//! also releases its proof when cancellation or unloading skips mod cleanup.

use std::sync::Arc;

use fastctx_hosts::claude::calls::{self, Proof};
use fastctx_types::ProcessIdentity;
use fastctx_types::ipc::Failure;

use crate::Node;

pub(super) fn recover(node: &Arc<Node>) {
    let _calls = node.claude.calls.lock().expect("never poisoned");
    match calls::retire_legacy(node.world.path()) {
        Ok(0) => {}
        Ok(count) => node.log.line(format_args!("removed {count} obsolete Claude call-proof files")),
        Err(error) => node.log.line(format_args!("retiring old Claude call proofs: {error}")),
    }
    let proofs = match calls::outstanding(node.world.path()) {
        Ok(proofs) => proofs,
        Err(error) => {
            node.log.line(format_args!("reading outstanding Claude calls: {error}"));
            return;
        }
    };
    for proof in proofs {
        let recovered = proof.and_then(|proof| {
            if fastctx_platform::is_alive(&proof.owner)? {
                observe(node, proof.tool_use_id, proof.owner)
            } else {
                calls::finish(node.world.path(), &proof.tool_use_id, &proof.owner).map(|_| ())
            }
        });
        if let Err(error) = recovered {
            node.log.line(format_args!("recovering an outstanding Claude call: {error}"));
        }
    }
}

pub(crate) fn begin(
    node: &Arc<Node>,
    tool_use_id: String,
    registration: serde_json::Value,
    owner: ProcessIdentity,
) -> Result<(), Failure> {
    let registration =
        serde_json::from_value(registration).map_err(|error| Failure::new(format!("Claude registration: {error}")))?;
    let proof = Proof { tool_use_id, registration, owner };
    super::pairing::validate_session(&proof.registration.session).map_err(|error| Failure::new(error.to_string()))?;
    {
        let _calls = node.claude.calls.lock().expect("never poisoned");
        calls::begin(node.world.path(), &proof)
            .map_err(|error| Failure::new(format!("claiming Claude call: {error}")))?;
    }
    if let Err(error) = observe(node, proof.tool_use_id.clone(), proof.owner.clone()) {
        finish(node, &proof.tool_use_id, &proof.owner)?;
        return Err(Failure::new(format!("watching the Claude call helper: {error}")));
    }
    Ok(())
}

pub(crate) fn finish(node: &Node, tool_use_id: &str, owner: &ProcessIdentity) -> Result<(), Failure> {
    let _calls = node.claude.calls.lock().expect("never poisoned");
    match calls::finish(node.world.path(), tool_use_id, owner) {
        Ok(true) => Ok(()),
        Ok(false) => Err(Failure::new("the Claude call is owned by a different helper")),
        Err(error) => Err(Failure::new(format!("releasing Claude call: {error}"))),
    }
}

fn observe(node: &Arc<Node>, id: String, owner: ProcessIdentity) -> std::io::Result<()> {
    let node = node.clone();
    // Long-lived waits must not occupy Tokio's blocking pool: tool execution
    // itself uses that pool and has to remain able to finish these calls.
    std::thread::Builder::new().name("claude-call-exit".into()).spawn(move || {
        if let Err(error) = fastctx_platform::host_process::wait_for_exit(&owner) {
            node.log.line(format_args!("watching Claude call {id}'s helper: {error}"));
            return;
        }
        let _calls = node.claude.calls.lock().expect("never poisoned");
        // Another invocation may already own this id; its proof stays intact.
        if let Err(error) = calls::finish(node.world.path(), &id, &owner) {
            node.log.line(format_args!("releasing exited Claude call {id}: {error}"));
        }
    })?;
    Ok(())
}
