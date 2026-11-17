//! Background lifecycle through Claude Code's CLI, without synthetic words.

use crate::Node;
use fastctx_hosts::claude::{background, exchange};
use fastctx_types::{ResidentId, ipc::Failure};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

pub(super) async fn run(node: &Arc<Node>, home: Option<&Path>, args: Vec<OsString>) -> Result<String, Failure> {
    let command = super::command(node, home)?;
    if args.first().is_some_and(|arg| arg == "stop" || arg == "rm") {
        let node = node.clone();
        return fastctx_platform::task::spawn_blocking(fastctx_platform::doing::carried(move || {
            super::retire_command::run(&node, &command, &args, std::time::Duration::from_secs(60))
        }))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(Failure::new);
    }
    run_command(command, args).await
}

async fn run_command(command: super::Command, args: Vec<OsString>) -> Result<String, Failure> {
    let super::Command { program, cwd, env } = command;
    fastctx_platform::task::spawn_blocking(fastctx_platform::doing::carried(move || {
        background::claude(&program, &args, &cwd, &env)
    }))
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(Failure::new)
}

/// Only supervisor records have a background management ID. The same CLI
/// also lists interactive sessions and unmanaged background processes.
pub async fn background_session(
    node: &Arc<Node>,
    home: Option<&Path>,
    session: &str,
) -> Result<Option<serde_json::Value>, Failure> {
    let listed = run(node, home, vec!["agents".into(), "--json".into(), "--all".into()]).await?;
    let agents: Vec<serde_json::Value> =
        serde_json::from_str(&listed).map_err(|error| Failure::new(error.to_string()))?;
    let found: Vec<_> = agents
        .iter()
        .filter(|agent| {
            agent["kind"].as_str() == Some("background")
                && agent["id"].as_str().is_some_and(|id| id.len() == 8 && id.bytes().all(|b| b.is_ascii_hexdigit()))
                && (agent["sessionId"].as_str() == Some(session) || agent["id"].as_str() == Some(session))
        })
        .collect();
    match found.as_slice() {
        [] => Ok(None),
        [agent] => Ok(Some((*agent).clone())),
        _ => Err(Failure::new("Claude Code identified more than one background session for this resident")),
    }
}

pub async fn stop(
    node: &Arc<Node>,
    home: Option<&Path>,
    resident: &ResidentId,
    session: &str,
    idle_only: bool,
) -> Result<(), Failure> {
    let agent = background_session(node, home, session).await?.ok_or_else(|| {
        Failure::new("Claude Code has no supervisor management ID for this session; its window was not closed")
    })?;
    if agent["pid"].as_u64() == Some(0) {
        return Err(Failure::new("Claude Code returned an invalid zero process ID; the session was not stopped"));
    };
    if agent["pid"].as_u64().is_none() {
        return settle_stopped(node, resident, session).await;
    }
    if idle_only && agent["status"].as_str() != Some("idle") {
        return Err(Failure::new(format!(
            "Claude Code reports status {}; only an idle background session can sleep",
            agent["status"]
        )));
    }
    let id = agent["id"].as_str().ok_or_else(|| Failure::new("Claude Code returned no background id"))?;
    run(node, home, vec!["stop".into(), id.into()]).await?;
    if super::running(node, home, session).await? {
        return Err(Failure::new("Claude Code accepted stop, but still reports a running background process"));
    }
    settle_stopped(node, resident, session).await
}

/// A completion written just before stop takes precedence over an inferred
/// interruption. Leave the reports for the ordinary consumer to acknowledge.
async fn settle_stopped(node: &Arc<Node>, resident: &ResidentId, session: &str) -> Result<(), Failure> {
    let directory = exchange::sessions(node.world.path()).join(session);
    let mut problems = exchange::recover_reports(&directory)
        .map_err(|error| Failure::new(format!("Claude is stopped; recovering its final reports: {error}")))?;
    let (reports, more) = exchange::harvest(&directory)
        .map_err(|error| Failure::new(format!("Claude is stopped; reading its final reports: {error}")))?;
    problems.extend(more);
    if !problems.is_empty() {
        return Err(Failure::new(format!(
            "Claude is stopped; its final reports could not be read: {}",
            problems.join("; ")
        )));
    }
    for report in reports {
        for event in report.events {
            if let exchange::Reported::Happened(event, _) = event
                && matches!(event.happened, fastctx_types::session::Happened::TurnEnded { .. })
            {
                crate::turns::apply(node, resident, event)
                    .await
                    .map_err(|error| Failure::new(format!("Claude is stopped; recording its final turn: {error}")))?;
            }
        }
    }
    super::ended_for_process(node, resident, session, Some("its background process is stopped".into()), None, None)
        .await
        .map_err(|error| Failure::new(error.to_string()))
}

/// Ends Claude Code's background session for good, when it has one: stops
/// it if it runs, idle only, and removes Claude Code's record of it
/// (`claude rm`), which keeps the session's own record; that goes with the
/// conversation.
pub async fn retire(node: &Arc<Node>, home: Option<&Path>, session: &str) -> Result<(), Failure> {
    let Some(agent) = background_session(node, home, session).await? else { return Ok(()) };
    let id = agent["id"].as_str().ok_or_else(|| Failure::new("Claude Code returned no background id"))?;
    if let Some(pid) = agent["pid"].as_u64() {
        if pid == 0 {
            return Err(Failure::new("Claude Code returned an invalid zero process ID"));
        }
        if agent["status"].as_str() != Some("idle") {
            return Err(Failure::new("only an idle background session can be deleted"));
        }
        run(node, home, vec!["stop".into(), id.into()]).await?;
        if super::running(node, home, session).await? {
            return Err(Failure::new("Claude Code accepted stop, but still reports a running background process"));
        }
    }
    // Deletion has already fenced this conversation. Final host reports go
    // with its records instead of being applied as a new turn after the fence.
    run(node, home, vec!["rm".into(), id.into()]).await?;
    Ok(())
}

pub async fn wake(node: &Arc<Node>, home: Option<&Path>, session: &str) -> Result<(), Failure> {
    let resident = node
        .residents
        .resident_of_thread(session)
        .or_else(|| session.get(..8).and_then(|short| node.residents.resident_of_thread(short)))
        .ok_or_else(|| Failure::new("the Claude session has no resident here"))?;
    let launch = match super::wake_once::begin(node, &resident, home, session).await? {
        super::wake_once::Admission::Launch(launch) => launch,
        super::wake_once::Admission::Present | super::wake_once::Admission::Waiting => return Ok(()),
    };
    let super::wake_once::Launch { intent, active: _active } = *launch;
    let result = wake_inner(node, &resident, home, session).await;
    let (started, accepted) = match &result {
        Ok(()) => (true, true),
        Err(error) => (error.started, false),
    };
    super::wake_once::finish(node, &resident, intent, started, accepted)?;
    result.map_err(|error| Failure::new(error.message))
}

async fn wake_inner(
    node: &Arc<Node>,
    resident: &ResidentId,
    home: Option<&Path>,
    session: &str,
) -> Result<(), background::CommandFailure> {
    let preflight = |error: Failure| background::CommandFailure { message: error.message, started: false };
    let attempted = |error: Failure| background::CommandFailure { message: error.message, started: true };
    if let Some(id) = node.residents.resident_of_thread(session)
        && let Some(source) = super::isolation::source(node, &id).map_err(preflight)?
    {
        super::isolation::continue_in(node, &id, &source, "").await?;
        return Ok(());
    }
    // A Claude Code older than the World takes is not started, as on every
    // other way a session starts.
    super::oldest::check(node, &super::command(node, home).map_err(preflight)?).await.map_err(preflight)?;
    if let Some(resident) = node.residents.resident_of_thread(session) {
        crate::profile::session::load_claude(node, &resident, false).await.map_err(preflight)?;
    }
    let mut command = super::command(node, home).map_err(preflight)?;
    // A foreground history has no saved supervisor cwd: it is resumed where
    // it was opened (`resume_dir`).
    command.cwd = super::resume_dir(node, session);
    super::trust(&[command.cwd.as_path()], &command.env)
        .map_err(|message| background::CommandFailure { message, started: false })?;
    let session_to_wake = session.to_string();
    let node_for_launch = node.clone();
    let active = super::wake_once::hold(node, resident).ok_or_else(|| preflight(super::wake_once::waiting()))?;
    let printed = fastctx_platform::task::spawn_blocking(move || {
        let _active = active;
        super::wake_command::run(
            node_for_launch.as_ref(),
            &command,
            &["--resume".into(), session_to_wake.into(), "--bg".into()],
        )
    })
    .await
    .map_err(|error| background::CommandFailure { message: error.to_string(), started: true })??;
    let started = background::started(&printed.out).ok_or_else(|| {
        attempted(Failure::new(format!("Claude Code did not identify the resumed background session: {}", printed.out)))
    })?;
    // Copied instead of woken, the copy goes at once. A session still
    // running takes what this wake was for; one that does not run Claude
    // Code would copy at every wake, so its conversation goes on in a
    // session of the World's (`continuing`).
    if !session.starts_with(&started) {
        super::copies::discard(node, home, &started).await.map_err(attempted)?;
        let seen = super::wake_once::observe(node, home, session).await.map_err(attempted)?;
        if !seen.resting {
            if seen.running {
                return Ok(());
            }
            return Err(attempted(super::wake_once::waiting()));
        }
        let resident = node
            .residents
            .resident_of_thread(session)
            .or_else(|| session.get(..8).and_then(|short| node.residents.resident_of_thread(short)))
            .ok_or_else(|| {
                Failure::new(format!(
                    "Claude Code copied {session} instead of waking it, and it is no conversation here"
                ))
            })
            .map_err(attempted)?;
        let why = background::copy_note(&printed.told).unwrap_or("it said nothing of why");
        let continued = super::continuing::start(node, &resident, home, session, "", why).await;
        continued?;
        return Ok(());
    }
    // The CLI receipt does not imply that the worker already registered.
    Ok(())
}
