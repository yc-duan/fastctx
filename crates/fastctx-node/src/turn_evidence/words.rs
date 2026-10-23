//! Compact factual text for reviewers; evidence paths always accompany summaries.

use fastctx_types::turn_evidence::TurnEvidence;
use std::fmt::Write;

pub(crate) fn render(evidence: &TurnEvidence, actions_path: &str) -> String {
    let mut text = String::new();
    let _ =
        writeln!(text, "Conversation {} · turn {}", evidence.resident, evidence.turn.as_deref().unwrap_or("unknown"));
    if let Some(stop) = &evidence.stop {
        let _ = writeln!(
            text,
            "Stopped: {:?}; category {}; message {}",
            stop.kind,
            stop.category.as_ref().map(ToString::to_string).unwrap_or_else(|| "unknown".into()),
            stop.message.as_deref().map(line).unwrap_or_default()
        );
    }
    for call in &evidence.calls {
        let _ = writeln!(
            text,
            "{} run {} -> {:?}; full command {}; output {}",
            call.facts.machine,
            line(&call.script),
            call.facts.state,
            call.source,
            call.facts.output.as_ref().map(|output| output.path.as_str()).unwrap_or("unavailable")
        );
    }
    for update in evidence.job_updates.iter().filter(|update| !update.primary) {
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
