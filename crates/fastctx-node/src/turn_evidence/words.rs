//! Compact factual text for reviewers; evidence paths always accompany summaries.

use fastctx_types::turn_evidence::TurnEvidence;
use std::fmt::Write;

pub(crate) fn render(evidence: &TurnEvidence, actions_path: &str) -> String {
    let mut text = String::new();
    // How the turn ended, with the host's category and words only where it gave them.
    if let Some(stop) = &evidence.stop {
        let kind = serde_json::to_value(stop.kind).ok().and_then(|kind| kind.as_str().map(str::to_owned)).unwrap_or_default();
        let category = stop.category.as_ref().map(|category| format!(" ({category})")).unwrap_or_default();
        let message = stop.message.as_deref().map(line).filter(|message| !message.is_empty());
        let _ = writeln!(text, "A turn ended: {kind}{category}{}", message.map(|message| format!(": {message}")).unwrap_or_default());
    }
    for call in &evidence.calls {
        let _ = writeln!(
            text,
            "{} run {} -> {:?}; job {}; call record {}; output {}",
            call.facts.machine,
            line(&call.script),
            call.facts.state,
            call.facts.job.as_ref().map(|job| job.as_str()).unwrap_or("not reported"),
            call.source,
            call.facts.output.as_ref().map(|output| output.path.as_str()).unwrap_or("see call record")
        );
    }
    for update in evidence.job_updates.iter().filter(|update| {
        !update.primary
            || !evidence
                .calls
                .iter()
                .any(|call| call.facts.machine == update.machine && call.facts.job.as_ref() == Some(&update.job))
    }) {
        let _ = writeln!(
            text,
            "{} job {} {} -> {:?}; full record {}; output {}",
            update.machine,
            update.job,
            update.script.as_deref().map(line).unwrap_or_else(|| "command unavailable".into()),
            update.state,
            update.record.as_deref().unwrap_or("unavailable"),
            update.output.as_ref().map(|output| output.path.as_str()).unwrap_or("not yet reported")
        );
    }
    let mut writes = std::collections::BTreeSet::new();
    for fact in evidence
        .calls
        .iter()
        .flat_map(|call| &call.facts.writes)
        .chain(evidence.job_updates.iter().flat_map(|update| &update.writes))
    {
        if !writes.insert((&fact.machine, &fact.id)) {
            continue;
        }
        let _ = writeln!(
            text,
            "{} {:?} {} +{} -{}; diff {}",
            fact.machine,
            fact.kind,
            fact.path,
            fact.added.map(|n| n.to_string()).unwrap_or_else(|| "unknown".into()),
            fact.removed.map(|n| n.to_string()).unwrap_or_else(|| "unknown".into()),
            fact.diff.as_ref().map(|diff| diff.path.as_str()).unwrap_or("unavailable")
        );
    }
    for action in evidence.actions.iter().filter(|action| action.hand != "run" || evidence.calls.is_empty()) {
        let _ = writeln!(
            text,
            "{} {} {} {} -> {}; full action {}",
            action.at,
            action.machine,
            action.hand,
            line(&action.what),
            line(&action.result),
            actions_path
        );
    }
    for event in &evidence.notices {
        if let fastctx_types::session::Happened::Heard { notice: Some(notice), .. } = &event.happened
            && let fastctx_types::residents::Notice::JobEnded { machine, job, how, output, .. } = notice.as_ref()
        {
            let _ = writeln!(text, "{} {} job {} ended {:?}; full output {}", event.at, machine, job, how, output);
        }
    }
    for handle in &evidence.waiting {
        let _ = writeln!(
            text,
            "Waiting: {} on {} · {:?} · since {} · {:?} · {}; output {}",
            handle.id,
            handle.machine,
            handle.kind,
            handle.since.map(|at| at.to_string()).unwrap_or_else(|| "unknown".into()),
            handle.liveness,
            line(&handle.summary),
            handle
                .output
                .as_ref()
                .map(|output| format!("{} bytes at {}", output.bytes, output.path))
                .unwrap_or_else(|| "none".into())
        );
    }
    text
}

fn line(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(240).collect();
    if chars.next().is_some() { format!("{head} …") } else { head }
}
