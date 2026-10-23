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
pub(super) const WAITING_CHANGED: &str = "The colleague's current goal changed what this conversation is waiting for.";
pub(super) const REVIEW_EXHAUSTED: &str = "Two consecutive sentinel suggestions were delivered; this stop is accepted.";
pub(super) const REVIEW_FAILED: &str = "The goal's sentinel could not judge this stop after three attempts.";
pub(super) const UNREVIEWED: &str = "No sentinel checks this goal, so the stop stands for the user to look at.";
pub(super) const RETRY_LIMIT: &str = "The goal has failed for six hours while this machine was online.";
pub(super) const QUOTA_RECOVERED: &str = "The goal's usage pause has ended.";
pub(super) const QUOTA_UNKNOWN: &str = "The host has not confirmed that ordinary usage is allowed.";

/// Why the previous turn stopped, as the host's category and its own words
/// where it gave them, for the turn that tries again.
pub(super) fn stopped(stop: &StopReason) -> String {
    // A category is a name, or an object of one name: `{"world": "host_exited"}`.
    let category = match &stop.category {
        Some(serde_json::Value::String(category)) => category.clone(),
        Some(serde_json::Value::Object(one)) if one.len() == 1 => {
            let (name, value) = one.iter().next().expect("one entry");
            value.as_str().map_or_else(|| name.clone(), str::to_owned)
        }
        Some(category) => category.to_string(),
        None => serde_json::to_value(stop.kind).ok().and_then(|kind| kind.as_str().map(str::to_owned)).unwrap_or_default(),
    };
    match stop.message.as_deref().map(str::trim).filter(|message| !message.is_empty()) {
        Some(message) => format!("The previous turn stopped with an error ({category}). The host said:\n{message}"),
        None => format!("The previous turn stopped with an error ({category})."),
    }
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
    use fastctx_types::turn_evidence::{Liveness, WaitKind};
    for handle in waiting {
        let kind = match handle.kind {
            WaitKind::Job => "job",
            WaitKind::Colleague => "colleague",
            WaitKind::Sentinel => "sentinel",
            WaitKind::Timer => "timer",
        };
        let state = match &handle.liveness {
            Liveness::Alive => "alive".to_string(),
            Liveness::Ended => "ended".to_string(),
            Liveness::Unknown { reason } => format!("not known now ({reason})"),
        };
        let mut line = format!("- {} on {} · {kind} · {} · {state}", handle.id, handle.machine, handle.summary);
        if let Some(at) = handle.since {
            let _ = write!(line, " · {} min", now.duration_since(at).as_secs().max(0) / 60);
        }
        let growth = handle.output_growth.map(|growth| format!(", +{growth}")).unwrap_or_default();
        if let Some(output) = &handle.output {
            let _ = write!(line, " · output {} ({} bytes{growth})", output.path, output.bytes);
        }
        let _ = writeln!(text, "{line}");
    }
    text.push_str("If they are moving, say so in a line and keep waiting. If one has stopped or is no longer\nneeded, it is yours to deal with.");
    text
}
