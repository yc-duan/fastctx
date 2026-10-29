//! What a session's mod never said, from what Claude Code keeps: where the
//! session's record is, and that its turn ended. The World hears each from
//! one report of the mod; when that report was lost (the node restarting as
//! the session started, a write that failed), the conversation could not be
//! read and its turn ran on for ever, with the user's next words queued
//! behind it. Claude Code's own facts settle them, never a time limit: the
//! World does not judge whether a running turn is stuck.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fastctx_hosts::claude::{ending, exchange, projects};
use fastctx_types::ResidentId;
use fastctx_types::residents::Activity;
use fastctx_types::session::{Happened, SessionEvent, TurnEnd, TurnHow};
use jiff::Timestamp;

use crate::Node;

/// How long an answer stands before it is settled here: the mod's own report
/// of it, if only late, comes first.
const REPORTED_FIRST: Duration = Duration::from_secs(60);

/// Names the record of a session whose mod never did, as the mod would have:
/// Claude Code keeps it under the login, in the folder named after the one
/// the session runs in. Nothing is named until that file is there and says
/// it is the session's.
pub(super) fn name_record(node: &Node, resident: &ResidentId, directory: &Path, session: &str) -> std::io::Result<()> {
    if exchange::transcript(directory)?.is_some() {
        return Ok(());
    }
    // A session's id names a file; one of any other shape names none.
    if session.is_empty() || !session.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-') {
        return Ok(());
    }
    let Some(registration) = exchange::registration(directory)? else { return Ok(()) };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(()) };
    let Some(home) = about.home.clone().or_else(crate::usage::claude_default) else { return Ok(()) };
    let Some(path) = projects::record(&home, registration.cwd.as_deref().map(Path::new), session)? else {
        return Ok(());
    };
    exchange::name_transcript(directory, &path)?;
    node.log.line(format_args!(
        "{session}'s mod never said where Claude Code keeps its record: it is {}, named for it",
        path.display()
    ));
    Ok(())
}

/// Ends, a while later, the turn the World still takes for running when
/// Claude Code's own facts say it ended and its mod never reported it: the
/// session's record ends with the model's answer, given after the turn began
/// and a minute or more ago, and Claude Code lists the session waiting for
/// its next prompt with nothing in flight. Asking Claude Code takes a while,
/// so it is done beside the look that called this.
pub(super) fn consider(node: &Arc<Node>, resident: &ResidentId, session: &str, directory: &Path) {
    let Some(now) = node.residents.now(resident) else { return };
    let Some(turn) = now.turn.clone().filter(|_| now.activity == Activity::Busy) else { return };
    if node.residents.about(resident).is_none_or(|(about, _)| about.session.as_deref() != Some(session)) {
        return;
    }
    let Ok(Some(record)) = exchange::transcript(directory) else { return };
    // A record still growing is a turn still going, or one whose report may
    // yet come.
    let quiet = std::fs::metadata(&record)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| SystemTime::now().duration_since(modified).is_ok_and(|age| age >= REPORTED_FIRST));
    if !quiet || !node.claude.settling.lock().expect("never poisoned").insert(session.to_string()) {
        return;
    }
    let (node, resident, session) = (node.clone(), resident.clone(), session.to_string());
    tokio::spawn(async move {
        settle(&node, &resident, &session, &turn, now.since, record).await;
        node.claude.settling.lock().expect("never poisoned").remove(&session);
    });
}

async fn settle(node: &Arc<Node>, resident: &ResidentId, session: &str, turn: &str, began: Timestamp, record: PathBuf) {
    let answered = match tokio::task::spawn_blocking(move || ending::answered(&record)).await {
        Ok(Ok(Some(answered))) => answered,
        Ok(Ok(None)) | Err(_) => return,
        Ok(Err(error)) => {
            node.log.line(format_args!("reading how {session}'s record ends: {error}"));
            return;
        }
    };
    let age = Timestamp::now().duration_since(answered.at);
    if answered.at < began || age < jiff::SignedDuration::try_from(REPORTED_FIRST).unwrap_or_default() {
        return;
    }
    let Some((about, _)) = node.residents.about(resident) else { return };
    match super::lifecycle::resting(node, &about).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            node.log.line(format_args!("asking Claude Code whether {session} is waiting: {}", error.message));
            return;
        }
    }
    let reading = super::reading(node, session);
    let _reading = reading.lock().await;
    // The mod's own report may have come meanwhile, or another turn begun.
    let current = node
        .residents
        .now(resident)
        .is_some_and(|now| now.activity == Activity::Busy && now.turn.as_deref() == Some(turn));
    if !current {
        return;
    }
    let end = TurnEnd {
        how: TurnHow::Completed,
        reason: None,
        error: None,
        duration_ms: u64::try_from(answered.at.duration_since(began).as_millis()).ok(),
        facts: None,
        last_message: answered.text,
    };
    let event = SessionEvent { at: answered.at, happened: Happened::TurnEnded { turn: turn.to_string(), end } };
    match crate::turns::apply(node, resident, event).await {
        Ok(()) => node.log.line(format_args!(
            "{resident}'s turn {turn} ended in Claude Code with no report from its mod; settled from its record and Claude Code's list"
        )),
        Err(error) => node.log.line(format_args!("settling {resident}'s turn {turn}: {error}")),
    }
}
