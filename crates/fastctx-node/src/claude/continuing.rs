//! A conversation whose Claude session Claude Code will not wake as itself
//! continues in a new session the World starts.
//!
//! Asked to resume a background session that does not run, Claude Code
//! wakes it in place only when nothing says it is open elsewhere (2.1.303).
//! Its records of open sessions naming a process it cannot see, as those
//! a login copied from another machine brings do, a job record it cannot
//! read, or one naming another session, and it starts a copy instead,
//! without the World's settings, at every wake (`copies`). The World does
//! not ask it again then: it forks the saved conversation into a new
//! background session with the conversation's plugin, settings and
//! instructions, gives it the words that were to wake the old one, and
//! follows the conversation there. The new session's record repeats what
//! it copied, which the history shows once (`history`).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fastctx_hosts::claude::{self, background};
use fastctx_types::ResidentId;

use crate::Node;

/// How long Claude Code has to start the session, as for a wake.
const WITHIN: Duration = Duration::from_secs(120);

/// Starts the session `resident` continues in from `session`, given `words`
/// when there are any, and records it as the conversation's; returns the
/// short id Claude Code printed for it. `why` explains why its saved startup
/// options cannot be used again. The caller counts a wake in
/// flight until this returns (`copies::Copies::waking`): a session reporting
/// meanwhile without naming its conversation, as the copy stopped before
/// this, waits for it. The new session names its conversation itself.
pub(super) async fn start(
    node: &Arc<Node>,
    resident: &ResidentId,
    home: Option<&Path>,
    session: &str,
    words: &str,
    why: &str,
) -> Result<String, background::CommandFailure> {
    let unstarted = |message: String| background::CommandFailure { message, started: false };
    let (about, _) = node.residents.about(resident).ok_or_else(|| unstarted(format!("{resident} was deleted")))?;
    let super::Command { program, env, .. } =
        super::command(node, home).map_err(|failure| unstarted(failure.message))?;
    // Where the session was opened, where Claude Code finds its record.
    let cwd = super::resume_dir(node, session);
    let assisting =
        crate::assists::host::starting(node, resident, &about.acting).map_err(|failure| unstarted(failure.message))?;
    let options = crate::session_options::read(node, resident).map_err(|failure| unstarted(failure.message))?;
    // Its settings name the conversation, as a new conversation's do.
    let mcp = crate::profile::session::claude(node, resident, false).map_err(|failure| unstarted(failure.message))?;
    let directory = node.world.conversation(resident);
    let (settings, instructions) = (directory.claude_settings(), directory.claude_instructions());
    let plugin = claude::plugin_dir(node.world.path());
    let world = node.world.path().to_path_buf();
    let (source, first_words) = (session.to_string(), words.to_string());
    let printed = tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
        let start = background::Start {
            plugin: &plugin,
            settings: &settings,
            mcp: &mcp,
            instructions: &instructions,
            first_words: &first_words,
            model: options.model.as_deref(),
            reasoning_effort: options.reasoning_effort.as_deref(),
            assisting,
        };
        super::trust(&[world.as_path(), cwd.as_path()], &env)
            .map_err(|message| background::CommandFailure { message, started: false })?;
        background::invoke(&program, &background::continue_args(&source, &start), &cwd, &env, WITHIN)
    }))
    .await
    .map_err(|error| background::CommandFailure {
        message: format!("continuing the session: {error}"),
        started: true,
    })??;
    let started = |message: String| background::CommandFailure { message, started: true };
    let short = background::started(&printed).ok_or_else(|| {
        started(format!(
            "Claude Code continued {session} without saying in which session; it printed: {}",
            printed.trim()
        ))
    })?;
    if session.starts_with(&short) {
        return Err(started(format!("Claude Code gave {session} itself for the session continuing it")));
    }
    // Known by the start of its id until its mod reports the whole of it,
    // which may have happened already.
    node.residents
        .set_session(&node.world, resident, &short, None, None)
        .map_err(|error| started(format!("recording that {resident} continues in {short}: {error}")))?;
    node.log
        .line(format_args!("Claude conversation {resident} continues from {session} in new session {short}: {why}"));
    Ok(short)
}
