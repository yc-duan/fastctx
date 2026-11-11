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

use fastctx_hosts::claude::{self, background};
use fastctx_types::ResidentId;

use crate::Node;

/// Starts the session `resident` continues in from `session`, given `words`
/// when there are any, and records it as the conversation's; returns the
/// short id Claude Code printed for it. `why` explains why its saved startup
/// options cannot be used again. The caller holds the resident's durable
/// launch intent until the replacement activation is reconciled. Its settings
/// name the conversation before the CLI returns the new session's id.
pub(super) async fn start(
    node: &Arc<Node>,
    resident: &ResidentId,
    home: Option<&Path>,
    session: &str,
    words: &str,
    why: &str,
) -> Result<String, background::CommandFailure> {
    let active = super::wake_once::hold(node, resident)
        .ok_or_else(|| background::CommandFailure { message: super::wake_once::waiting().message, started: false })?;
    let (node, resident, home, source, first_words) =
        (node.clone(), resident.clone(), home.map(Path::to_path_buf), session.to_string(), words.to_string());
    let recording = node.clone();
    let recorded_id = resident.clone();
    let printed = fastctx_platform::task::spawn_blocking(move || {
        let _active = active;
        let unstarted = |message: String| background::CommandFailure { message, started: false };
        let (about, _) = node.residents.about(&resident).ok_or_else(|| unstarted(format!("{resident} was deleted")))?;
        let super::Command { program, env, .. } =
            super::command(&node, home.as_deref()).map_err(|failure| unstarted(failure.message))?;
        // Where the session was opened, where Claude Code finds its record.
        let cwd = super::resume_dir(&node, &source);
        let assisting = crate::assists::host::starting(&node, &resident, &about.acting)
            .map_err(|failure| unstarted(failure.message))?;
        let options = crate::session_options::read(&node, &resident).map_err(|failure| unstarted(failure.message))?;
        // Its settings name the conversation, as a new conversation's do.
        let mcp =
            crate::profile::session::claude(&node, &resident, false).map_err(|failure| unstarted(failure.message))?;
        if let Err(error) = super::reword(&node, &resident) {
            node.log.line(format_args!("rewording {resident}'s instructions before it continues: {error}"));
        }
        let directory = node.world.conversation(&resident);
        let (settings, instructions) = (directory.claude_settings(), directory.claude_instructions());
        let plugin = claude::plugin_dir(node.world.path());
        let world = node.world.path().to_path_buf();
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
        super::wake_command::run(
            &node,
            &super::Command { program, cwd, env },
            &background::continue_args(&source, &start),
        )
        .map(|printed| printed.out)
    })
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
    let (updating, id, next) = (recording.clone(), recorded_id.clone(), short.clone());
    fastctx_platform::task::spawn_blocking(move || {
        updating.residents.set_session(&updating.world, &id, &next, None, None)
    })
    .await
    .map_err(|error| started(error.to_string()))?
    .map_err(|error| started(format!("recording that {recorded_id} continues in {short}: {error}")))?;
    recording
        .log
        .line(format_args!("Claude conversation {recorded_id} continues from {session} in new session {short}: {why}"));
    Ok(short)
}
