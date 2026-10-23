//! Runtime facts and review text, kept together for the shared editorial pass.

use fastctx_types::goals::runtime::{ReviewCause, Verdict};
use fastctx_types::turn_evidence::StopReason;

pub(super) const OPENED: &str = "The goal is on.";
pub(super) const USER_STOPPED: &str = "The user stopped this turn; the goal is paused.";
pub(super) const COMPACTED: &str =
    "The previous turn stopped because its context did not fit. The conversation has been compacted.";
pub(super) const CONTEXT_AGAIN: &str = "The first turn after compaction could not fit its context.";
pub(super) const IDLE: &str =
    "The turn ended with no verified work to wake it; checking ten seconds of continuous idle.";
pub(super) const WAITING: &str = "Waiting on work that will wake this conversation.";
pub(super) const REVIEW_EXHAUSTED: &str = "Two consecutive sentinel suggestions were delivered; this stop is accepted.";
pub(super) const REVIEW_FAILED: &str = "The goal's sentinel could not judge this stop after three attempts.";
pub(super) const UNREVIEWED: &str = "No sentinel checks this goal, so the stop stands for the user to look at.";
pub(super) const RETRY_LIMIT: &str = "The goal has failed for six hours while this machine was online.";
pub(super) const QUOTA_RECOVERED: &str = "The goal's usage pause has ended.";
pub(super) const QUOTA_UNKNOWN: &str = "The host has not confirmed that ordinary usage is allowed.";

pub(super) fn stopped(stop: &StopReason) -> String {
    let category = stop.category.as_ref().map(ToString::to_string).unwrap_or_else(|| format!("{:?}", stop.kind));
    format!(
        "The previous turn stopped: {category}.\n{}",
        stop.message.as_deref().unwrap_or("No host message was reported.")
    )
}

pub(super) fn review(cause: ReviewCause, verdict: &Verdict) -> String {
    match verdict {
        Verdict::Sound { reason } | Verdict::Failed { reason } => reason.clone(),
        Verdict::Advice { looked, saw, thinks } => {
            let why = match cause {
                ReviewCause::Idle => "with the goal still on and nothing to wake you",
                ReviewCause::Blocked => "when you marked it blocked",
                ReviewCause::Paused => "when you marked it paused",
            };
            format!(
                "The goal's sentinel, a smaller model that checks this goal while the user is away, looked\nafter your turn ended {why}.\nIt looked at: {looked}\nIt saw: {saw}\nIt thinks: {thinks}\nThis is a suggestion; you decide."
            )
        }
    }
}

pub(super) fn reminder(
    since: jiff::Timestamp,
    now: jiff::Timestamp,
    waiting: &[fastctx_types::turn_evidence::WaitHandle],
) -> String {
    use std::fmt::Write;
    let mut text = format!(
        "The goal is still on, and you have been waiting {} min on:\n",
        now.duration_since(since).as_secs().max(0) / 60
    );
    for handle in waiting {
        let elapsed = handle
            .since
            .map(|at| format!("{} min", now.duration_since(at).as_secs().max(0) / 60))
            .unwrap_or_else(|| "duration unknown".into());
        let output = handle
            .output
            .as_ref()
            .map(|output| format!("{} bytes; complete output and latest lines: {}", output.bytes, output.path))
            .unwrap_or_else(|| "no output file reported".into());
        let growth =
            handle.output_growth.map(|growth| format!("+{growth} bytes")).unwrap_or_else(|| "growth unknown".into());
        let _ = writeln!(
            text,
            "- {} on {} · {:?} · {} · {:?} · {}; {}; {}",
            handle.id, handle.machine, handle.kind, handle.summary, handle.liveness, elapsed, growth, output
        );
    }
    text.push_str("If they are moving, say so in a line and keep waiting. If one has stopped or is no longer\nneeded, it is yours to deal with.");
    text
}
