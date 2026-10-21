//! The user's word on Claude Code's bypass-permissions consent, carried to
//! the machine it is about. Only the user gives it, on one of the user's own
//! devices (`world-projects.md`); the machine records who and when.

use fastctx_types::access::Written;
use std::path::PathBuf;
use std::sync::Arc;

use fastctx_install::apply;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};

use crate::Node;

/// What Claude Code on `machine`, in its home `home` or its default one,
/// records once the word is given, and whether it changed.
pub async fn call(
    node: &Arc<Node>,
    machine: Written,
    accept: bool,
    home: Option<PathBuf>,
) -> Result<(bool, bool), Failure> {
    if node.is_here(&machine) {
        return here(node, accept, home, format!("the user on {machine}")).await;
    }
    let call = RemoteCall::ClaudeConsent { accept, home };
    match node
        .call(&Acting::Here, &machine, Purpose::Agents, call)
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    {
        RemoteReply::ClaudeConsent { accepted, changed } => Ok((accepted, changed)),
        _ => Err(Failure::new(format!("{machine} answered something other than Claude Code's consent"))),
    }
}

/// Writes the word into the settings of this user's Claude Code home here
/// written `home`, or of its default one. `by` names whose device it came
/// from, for the node's log.
pub async fn here(node: &Arc<Node>, accept: bool, home: Option<PathBuf>, by: String) -> Result<(bool, bool), Failure> {
    let local = node.clone();
    let (changed, dir) = tokio::task::spawn_blocking(move || {
        let env = crate::claude::environment().map_err(|failure| failure.message)?;
        let user_home = fastctx_platform::user_home().map_err(|error| error.to_string())?;
        let dir = match home {
            Some(dir) => {
                let agent = fastctx_types::AgentName::try_from("claude").expect("an agent name");
                crate::logins::known(&local, &agent, &dir).map_err(|failure| failure.message)?;
                dir
            }
            None => crate::homes::claude_default(&env, &user_home),
        };
        let places = crate::agents::places(&local, &env, &user_home, None);
        let changed = apply::consent_claude_at(&places, &dir, accept).map_err(|error| error.to_string())?;
        Ok::<_, String>((changed, dir))
    })
    .await
    .map_err(|error| Failure::new(format!("recording Claude Code's consent: {error}")))?
    .map_err(|error| Failure::new(format!("recording Claude Code's consent: {error}")))?;
    if changed {
        let word = if accept { "accepted" } else { "withdrew" };
        node.log.line(format_args!(
            "{by} {word} Claude Code's bypass permissions mode for its home {} on this machine",
            dir.display()
        ));
    }
    Ok((accept, changed))
}
