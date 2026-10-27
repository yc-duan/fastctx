//! What the World tells a resident, as the resident reads it: the end of a
//! job it started, a colleague's answer, words someone said to it.
//!
//! Each is one block of its own whose first line is a head note saying who
//! or what it is about and where (`world-colleagues.md`, 两个工具). A
//! colleague's answer is its last message verbatim, between a head note
//! saying how the turn ended and a last line pointing at what it did.

use fastctx_types::residents::{Notice, STOPPED_BY_USER};
use fastctx_types::run::EndedHow;
use fastctx_types::session::{Speaker, TurnEnd, TurnHow};

use crate::run::present::{display_path, status_text};

/// A notice as one block of text.
pub fn told(notice: &Notice) -> String {
    match notice {
        Notice::Sentinel { observation } => {
            fastctx_types::sentinel_words::notification(observation, jiff::Timestamp::now())
        }
        Notice::JobEnded {
            job,
            machine,
            how,
            status,
            at,
            output,
            stderr,
            last,
            stop,
            follower,
            follower_unconfirmed,
            ..
        } => {
            let how = match (how, status) {
                (EndedHow::Finished, Some(status)) => status_text(*status),
                (EndedHow::Finished, None) => "finished".to_string(),
                // Said so the model does not take it for a failure to retry.
                (EndedHow::Stopped, _) if stop.as_ref().is_some_and(|stop| stop.by == STOPPED_BY_USER) => {
                    "stopped by the user".to_string()
                }
                (EndedHow::Stopped, _) => "stopped".to_string(),
                (EndedHow::Interrupted { reason }, _) => format!("interrupted: {reason}"),
                (EndedHow::Failed { error }, _) => format!("failed: {error}"),
            };
            // A notice is written as it is delivered. Its reader knows no
            // machine's clock, so a late one says how long ago, as `w` does;
            // within a minute the age changes nothing it would do.
            let waited = jiff::Timestamp::now().as_second().saturating_sub(at.as_second());
            let ago = if waited >= 60 { format!(" {} ago", duration(waited as u64)) } else { String::new() };
            let errors = stderr.as_ref().map(|path| format!("; stderr in {}", display_path(path))).unwrap_or_default();
            let head = format!(
                "=== {job} on {machine} ended ({how}){ago}; its output is in {}{errors} ===",
                display_path(output)
            );
            let mut text = match last {
                Some(last) if last.whole => format!("{head}\n{}", last.text),
                Some(last) => format!("{head}\n=== its last lines ===\n{}", last.text),
                None => head,
            };
            if *follower_unconfirmed && let Some(follower) = follower {
                text.push_str(&format!("\nIts follower on {follower} could not be asked whether it took this end; the same end may also be in the output of the command that ran on {follower}."));
            }
            text
        }
        Notice::Answer { colleague, end, .. } => {
            let how = turn_end(end);
            let message = end.last_message.as_deref().unwrap_or("(it ended the turn without a message)");
            format!(
                "=== colleague {} on {} ({how}) ===\n{message}\n=== what it did: fastctx w {} ===",
                colleague.resident, colleague.machine, colleague.resident
            )
        }
        Notice::Health { colleague, health, .. } => format!(
            "=== colleague {} on {} (in its turn) ===\n{}\n=== what it is doing: fastctx w {} ===",
            colleague.resident, colleague.machine, health.text, colleague.resident
        ),
        Notice::Question { text, .. } => text.clone(),
        Notice::Words { text, goal_report: Some(_), .. } | Notice::Words { text, goal_question: Some(_), .. } => {
            text.clone()
        }
        Notice::Words { from, text, .. } => words(from, text),
        Notice::Attached { message, text } => message.render(text),
        Notice::World { text, .. } => words(&Speaker::World, text),
    }
}

/// The colleague's original conclusion, with only a factual envelope around it.
pub fn goal_report(report: &fastctx_types::goals::Report) -> String {
    let elapsed = report.ended_at.as_second().saturating_sub(report.started_at.as_second()).max(0) as u64;
    let resume = report.resume_at.map(|at| format!(", resumes at {at}")).unwrap_or_default();
    format!(
        "=== colleague {} on {} (goal {}, {}{resume}) ===\n{}\n=== what it did: fastctx w {} ===",
        report.colleague.resident,
        report.colleague.machine,
        goal_status(report.status),
        duration(elapsed),
        report.text,
        report.colleague.resident
    )
}

/// Words for a session, under a head note saying who they are from, so the
/// reader knows who is speaking (`world-colleagues.md`, 两个工具).
pub fn words(from: &Speaker, text: &str) -> String {
    let from = match from {
        Speaker::Page { page } => page.to_string(),
        Speaker::User { machine } => format!("the user, on {machine}"),
        Speaker::Resident { address } => format!("{} on {}", address.resident, address.machine),
        Speaker::World => "fastctx".to_string(),
    };
    format!("=== from {from} ===\n{text}")
}

/// Whether words in a session are something the World told it: everything
/// the World says begins with a head note (`words`, `told`), and words a
/// person typed into the host's own window do not.
pub fn is_told(text: &str) -> bool {
    text.starts_with("=== ")
}

/// How a turn ended and how long it ran, for a head note:
/// `turn completed, 14m`, `turn failed: <reason>`.
pub fn turn_end(end: &TurnEnd) -> String {
    let mut how = match (&end.how, &end.reason) {
        (TurnHow::Completed, _) => "turn completed".to_string(),
        (how, Some(reason)) => format!("turn {how}: {reason}"),
        (how, None) => format!("turn {how}"),
    };
    if let Some(ms) = end.duration_ms {
        how.push_str(", ");
        how.push_str(&duration(ms / 1000));
    }
    how
}

/// The goal's state as shown with its original report.
pub fn goal_status(status: fastctx_types::goals::Status) -> &'static str {
    match status {
        fastctx_types::goals::Status::Active => "active",
        fastctx_types::goals::Status::Complete => "complete",
        fastctx_types::goals::Status::Blocked => "blocked",
        fastctx_types::goals::Status::Paused => "paused",
    }
}

/// A span of seconds as people say it: `35s`, `14m`, `2h10m`, `3d4h`.
pub fn duration(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => match (seconds / 3600, seconds % 3600 / 60) {
            (hours, 0) => format!("{hours}h"),
            (hours, minutes) => format!("{hours}h{minutes}m"),
        },
        _ => match (seconds / 86_400, seconds % 86_400 / 3600) {
            (days, 0) => format!("{days}d"),
            (days, hours) => format!("{days}d{hours}h"),
        },
    }
}
