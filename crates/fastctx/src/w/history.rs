use super::*;
use fastctx_types::since::{MachineSince, SinceEvent, SinceKind, SinceTown, WorldSince};

// The overview shows a few dozen facts; --full returns the complete range.
const RECENT: usize = 40;

pub(super) fn parse(text: &str, now: Timestamp) -> anyhow::Result<Timestamp> {
    if let Ok(at) = text.parse::<Timestamp>() {
        return Ok(at);
    }
    if let Ok(time) = text.parse::<jiff::civil::Time>() {
        let zone = jiff::tz::TimeZone::system();
        let mut date = now.to_zoned(zone.clone()).date();
        let mut at = date.to_datetime(time).to_zoned(zone.clone())?.timestamp();
        if at > now {
            date = date.checked_sub(jiff::Span::new().days(1))?;
            at = date.to_datetime(time).to_zoned(zone)?.timestamp();
        }
        return Ok(at);
    }
    let elapsed: jiff::SignedDuration = text.parse().map_err(|error| {
        anyhow::anyhow!("invalid --since {text:?}: expected 2h, 14:00 or an RFC 3339 timestamp ({error})")
    })?;
    anyhow::ensure!(!elapsed.is_negative(), "--since duration must not be negative");
    Ok(now.checked_sub(elapsed)?)
}

pub(super) fn render(history: &WorldSince, previous: bool, full: bool, now: Timestamp, view: &TownView) -> String {
    let mut lines = Vec::new();
    for event in &history.events {
        if let SinceKind::ResidentBorn { resident, origin: Origin::User, .. } = &event.what
            && view.known(resident).is_none()
        {
            continue;
        }
        // A job done within its `run` call was answered to its caller there:
        // routine work, in that resident's actions. One that went on in the
        // background is news.
        if let SinceKind::JobEnded { elapsed_ms, .. } = &event.what
            && u128::try_from(*elapsed_ms).is_ok_and(|ms| ms < fastctx_hands::run::WAIT.as_millis())
        {
            continue;
        }
        // Registration refreshes and delivery receipts are not new work.
        // A turn end already says who was answered; it is not repeated here.
        // Journals are project memory, not wired in until it is finished.
        if matches!(
            event.what,
            SinceKind::ClaudeRegistered { .. } | SinceKind::JournalEntry { .. } | SinceKind::JournalDescription { .. }
        ) || matches!(&event.what, SinceKind::AnswerDelivery { outcome, .. }
                if !matches!(outcome, fastctx_types::residents::Outcome::Failed { .. }
                    | fastctx_types::residents::Outcome::Waiting { why: Some(_) }))
        {
            continue;
        }
        lines.push(format!(
            "  {}  {}  {}{}\n",
            when(event.at, now),
            fastctx_types::observation::written(&event.machine, event.source.as_ref()),
            fact(event),
            if history.town.is_some()
                && matches!(&event.town, SinceTown::Unrecorded { recorded_in } if !recorded_in.is_empty())
            {
                " (original project unrecorded)"
            } else {
                ""
            }
        ));
    }
    let mut issues = history
        .issues
        .iter()
        .map(|issue| format!("  could not read {}: {}\n", issue.source, issue.reason))
        .collect::<String>();
    for machine in &history.machines {
        if let Some(observation) = machine.observation() {
            for missing in &observation.unanswered {
                issues.push_str(&format!("  {}: {missing}\n", machine.written()));
            }
        }
        match machine {
            MachineSince::Answered { reply, .. } => {
                for issue in &reply.issues {
                    issues.push_str(&format!(
                        "  {} could not read {}: {}\n",
                        machine.written(),
                        issue.source,
                        issue.reason
                    ));
                }
            }
            MachineSince::NoAnswer { machine, reason, .. } if reason.starts_with("answered current activity") => {
                issues.push_str(&format!("  {machine}: {reason}\n"))
            }
            _ => {}
        }
    }
    if lines.is_empty() && issues.is_empty() {
        return String::new();
    }
    let from = if previous { "your last turn ended, " } else { "" };
    let mut text = format!("since {from}{}\n", when(history.since, now));
    if !full && lines.len() > RECENT {
        let town = history.town.as_ref().map(|town| format!(" {town}")).unwrap_or_default();
        text.push_str(&format!(
            "  last {RECENT} of {} (all: fastctx w{town} --since '{}' --full)\n",
            lines.len(),
            history.since
        ));
    }
    let skip = if full { 0 } else { lines.len().saturating_sub(RECENT) };
    for line in lines.into_iter().skip(skip) {
        text.push_str(&line);
    }
    text.push_str(&issues);
    text
}

fn fact(event: &SinceEvent) -> String {
    let who = |resident: &Address| render::person(resident, &event.machine);
    match &event.what {
        SinceKind::FileAction { resident, hand, paths, result } => {
            format!("{} {hand} {paths} -> {result}", who(resident))
        }
        SinceKind::ResidentBorn { resident, .. } => format!("{} first appeared", who(resident)),
        SinceKind::TurnEnded { resident, end, readers, .. } => {
            let readers = readers
                .as_ref()
                .filter(|readers| !readers.is_empty())
                .map(|readers| format!(" for {}", readers.iter().map(who).collect::<Vec<_>>().join(", ")))
                .unwrap_or_default();
            format!("{} {}{readers}", who(resident), turn_end(end))
        }
        SinceKind::JobEnded { job, owner, how, exit, script, .. } => {
            let end = match how {
                EndedHow::Finished => exit
                    .as_ref()
                    .map(|exit| detail::status(exit.status))
                    .unwrap_or_else(|| "finished; exit unrecorded".into()),
                EndedHow::Stopped => "stopped".into(),
                EndedHow::Interrupted { reason } => format!("interrupted: {reason}"),
                EndedHow::Failed { error } => format!("failed: {error}"),
            };
            format!("{job} ended, {end} ({owner}): {}", first_line(script))
        }
        SinceKind::ColleagueAsked { asker, colleague, label } => {
            format!("{} asked {}: \"{label}\"", who(asker), who(colleague))
        }
        SinceKind::ResidentMoved { resident, from, to } => format!(
            "{} moved from {} to {}",
            who(resident),
            from.as_ref().map_or("outside", |t| t.as_str()),
            to.as_ref().map_or("outside", |t| t.as_str())
        ),
        SinceKind::SubmissionUncertain { resident, why, .. } => {
            format!("{}: submission unconfirmed ({why})", who(resident))
        }
        SinceKind::JournalEntry { .. } | SinceKind::JournalDescription { .. } | SinceKind::ClaudeRegistered { .. } => {
            unreachable!()
        }
        SinceKind::ProjectVersion { version, name, .. } => format!("project \"{name}\" v{version}"),
        SinceKind::MachineConnected => "came online".into(),
        SinceKind::MachineOffline => "went offline".into(),
        SinceKind::AnswerDelivery { colleague, to, outcome, .. } => {
            let reason = match outcome {
                fastctx_types::residents::Outcome::Failed { reason } => reason,
                fastctx_types::residents::Outcome::Waiting { why: Some(why) } => why,
                _ => unreachable!(),
            };
            format!("{} could not deliver its answer to {}: {reason}", who(colleague), who(to))
        }
    }
}
