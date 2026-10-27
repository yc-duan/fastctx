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
        return tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
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
    tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
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
pub async fn retire(
    node: &Arc<Node>,
    home: Option<&Path>,
    resident: &ResidentId,
    session: &str,
) -> Result<(), Failure> {
    let Some(agent) = background_session(node, home, session).await? else { return Ok(()) };
    if agent["pid"].as_u64().is_some() {
        stop(node, home, resident, session, true).await?;
    }
    let id = agent["id"].as_str().ok_or_else(|| Failure::new("Claude Code returned no background id"))?;
    run(node, home, vec!["rm".into(), id.into()]).await?;
    Ok(())
}

pub async fn wake(node: &Arc<Node>, home: Option<&Path>, session: &str) -> Result<(), Failure> {
    let record = background_session(node, home, session).await?;
    let session = match record.as_ref() {
        Some(record) => record["sessionId"]
            .as_str()
            .ok_or_else(|| Failure::new("Claude Code did not provide the background session's full conversation ID"))?,
        // A closed foreground conversation has a transcript but no supervisor record.
        None => session,
    };
    if super::running(node, home, session).await? {
        return Ok(());
    }
    // A Claude Code older than the World takes is not started, as on every
    // other way a session starts.
    super::oldest::check(node, &super::command(node, home)?).await?;
    if let Some(resident) = node.residents.resident_of_thread(session) {
        crate::profile::session::claude(node, &resident, false)?;
    }
    let mut command = super::command(node, home)?;
    // A foreground history has no saved supervisor cwd: it is resumed where
    // it was opened (`resume_dir`).
    command.cwd = super::resume_dir(node, session);
    super::trust(&[command.cwd.as_path()], &command.env).map_err(Failure::new)?;
    // A session that reports while this runs waits to learn whether it is a copy.
    let waking = node.claude.copies.waking();
    let output = run_command(command, vec!["--resume".into(), session.into(), "--bg".into()]).await?;
    let started = background::started(&output).ok_or_else(|| {
        Failure::new(format!("Claude Code did not identify the resumed background session: {output}"))
    })?;
    // Still running, it was copied instead: the copy goes at once, and the
    // session it copied, running, takes what this wake was for.
    if !session.starts_with(&started) {
        super::copies::discard(node, home, &started).await;
        if super::running(node, home, session).await? {
            return Ok(());
        }
        return Err(Failure::new(format!(
            "{session} was still running; Claude Code started a copy of it instead, which was stopped"
        )));
    }
    drop(waking);
    if !super::running(node, home, session).await? {
        return Err(Failure::new("Claude Code accepted resume but does not report the original session running"));
    }
    Ok(())
}
