//! The user's word on Claude Code's bypass-permissions consent, carried to
//! the machine it is about. Only the user gives it, on one of the user's own
//! devices (`world-projects.md`); the machine records who and when.

use std::sync::Arc;

use fastctx_install::apply;
use fastctx_types::MachineName;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};

use crate::Node;

/// What Claude Code on `machine` records once the word is given, and
/// whether it changed.
pub async fn call(node: &Arc<Node>, machine: MachineName, accept: bool) -> Result<(bool, bool), Failure> {
    if machine == node.machine() {
        return here(node, accept, format!("the user on {machine}")).await;
    }
    let link = node.link().ok_or_else(|| Failure::new("this machine belongs to no connected World"))?;
    match link
        .call(&machine, RemoteCall::ClaudeConsent { accept })
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    {
        RemoteReply::ClaudeConsent { accepted, changed } => Ok((accepted, changed)),
        _ => Err(Failure::new(format!("{machine} answered something other than Claude Code's consent"))),
    }
}

/// Writes the word into this user's Claude Code settings here. `by` names
/// whose device it came from, for the node's log.
pub async fn here(node: &Arc<Node>, accept: bool, by: String) -> Result<(bool, bool), Failure> {
    let local = node.clone();
    let changed = tokio::task::spawn_blocking(move || {
        let env = crate::claude::environment().map_err(|failure| failure.message)?;
        let home = fastctx_platform::user_home().map_err(|error| error.to_string())?;
        let places = crate::agents::places(&local, &env, &home, None);
        apply::consent_claude(&places, accept).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| Failure::new(format!("recording Claude Code's consent: {error}")))?
    .map_err(|error| Failure::new(format!("recording Claude Code's consent: {error}")))?;
    if changed {
        let word = if accept { "accepted" } else { "withdrew" };
        node.log.line(format_args!("{by} {word} Claude Code's bypass permissions mode for this machine"));
    }
    Ok((accept, changed))
}
