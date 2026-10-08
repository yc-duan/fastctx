use super::*;

pub(super) fn detail(looked: &LookedUp, now: Timestamp) -> String {
    let mut text = String::new();
    if looked.found.is_empty() {
        text.push_str(&format!("=== w {}: no answering machine has it ===\n", looked.id));
    } else if looked.found.len() > 1 {
        let machines = looked
            .found
            .iter()
            .map(|found| match found {
                Looked::Resident(detail) => detail.resident.about.machine.as_str(),
                Looked::Job { record } => record.machine.as_str(),
            })
            .collect::<Vec<_>>();
        text.push_str(&format!("{} exists on {}; use {}@machine.\n", looked.id, machines.join(", "), looked.id));
    } else {
        for found in &looked.found {
            match found {
                Looked::Resident(detail) => text.push_str(&resident_detail(detail, now)),
                Looked::Job { record } => text.push_str(&job_detail(record, now)),
            }
        }
    }
    // Only when nothing answered with it can it be on a machine that did not
    // answer; a random id found once does not need the rest of the World.
    if looked.found.is_empty() && !looked.unasked.is_empty() {
        text.push_str(&format!("not answered: {}\n", looked.unasked.join(", ")));
    }
    for job in &looked.jobs {
        text.push_str(&format!(
            "{} on {}, {} ({}): {}\n  output: {} {}\n",
            job.job,
            job.machine,
            ago(job.started_at, now),
            job.owner,
            job.command,
            job.machine,
            shown(&job.output)
        ));
        if let Some(stderr) = &job.stderr {
            text.push_str(&format!("         stderr: {} {}\n", job.machine, shown(stderr)));
        }
    }
    text
}

/// A path on any machine, written as the World writes paths, `C:/…` on
/// Windows; World directories hold no other backslashes.
fn shown(path: &str) -> String {
    path.replace('\\', "/")
}

fn record_path(base: &str, file: &str) -> String {
    format!("{}/{file}", shown(base))
}

fn resident_detail(detail: &ResidentDetail, now: Timestamp) -> String {
    let resident = &detail.resident;
    let about = &resident.about;
    let machine = &about.machine;
    let mut agent = host(&about.host);
    if !about.host.version.is_empty() {
        agent.push_str(&format!(" {}", about.host.version));
    }
    if let Some(model) = &about.model {
        agent.push_str(&format!(", model {model}"));
    }
    let origin = match &about.origin {
        Origin::Colleague { asked_by, .. } => format!(", for {}", render::person(asked_by, machine)),
        Origin::User => String::new(),
    };
    let mut text =
        format!("=== w {}: {} on {machine}{origin}, first seen {} ago ===\n", about.id, agent, ago(about.born_at, now));
    match &about.origin {
        Origin::Colleague { label, .. } => text.push_str(&format!(
            "task     \"{label}\"   ({})\n",
            if detail.asked.is_some() {
                format!("full request: {machine} {}", record_path(&detail.records, "messages"))
            } else {
                "full request unrecorded".into()
            }
        )),
        Origin::User => {
            if let Some(title) = resident.now.as_ref().and_then(|state| state.title.as_ref()) {
                text.push_str(&format!("title    \"{title}\"\n"));
            }
        }
    }
    if !resident.here {
        text.push_str("session  not open now\n");
    }
    let step = render::step(resident, now);
    if !step.is_empty() {
        text.push_str(&format!("now      {step}\n"));
    }
    for job in &detail.jobs {
        text.push_str(&format!(
            "         {} on {}, {}: {}\n         output: {} {}\n",
            job.job,
            job.machine,
            ago(job.started_at, now),
            job.command,
            job.machine,
            shown(&job.output)
        ));
        if let Some(stderr) = &job.stderr {
            text.push_str(&format!("         stderr: {} {}\n", job.machine, shown(stderr)));
        }
    }
    if detail.actions_total > 0 {
        text.push_str(&format!(
            "actions  last {} of {} (all: {machine} {})\n",
            detail.actions.len(),
            detail.actions_total,
            record_path(&detail.records, "actions")
        ));
        for action in &detail.actions {
            let elsewhere = if action.machine == *machine { String::new() } else { format!("  {}", action.machine) };
            text.push_str(&format!(
                "  {}{elsewhere}  {}  {} -> {}\n",
                when(action.at, now),
                action.hand,
                action.what,
                action.result
            ));
        }
    }
    if matches!(about.origin, Origin::Colleague { .. }) {
        if detail.answers.is_empty() {
            text.push_str("answers  none yet\n");
        } else {
            text.push_str(&format!("answers  (full: {machine} {})\n", record_path(&detail.records, "messages")));
            for line in &detail.answers {
                if let MessageKind::Ended { end, .. } = &line.what {
                    let message = end
                        .last_message
                        .as_ref()
                        .map(|message| format!(": {}", first_line(message)))
                        .unwrap_or_default();
                    text.push_str(&format!("  {} {}{message}\n", when(line.at, now), turn_end(end)));
                }
            }
        }
    }
    text
}

fn job_detail(record: &JobRecord, now: Timestamp) -> String {
    let owner = match record.owner.resident() {
        Some(address) => render::person(address, &record.machine),
        None => record.owner.to_string(),
    };
    let mut text = format!(
        "=== w {}: on {}, started by {owner} {} ago ===\n",
        record.id,
        record.machine,
        ago(record.started_at, now)
    );
    let state = match (&record.end, &record.exit) {
        (Some(end), exit) => {
            let how = match (&end.how, exit) {
                (EndedHow::Finished, Some(exit)) => status(exit.status),
                (EndedHow::Finished, None) => "finished".to_string(),
                (EndedHow::Stopped, _) => "stopped".to_string(),
                (EndedHow::Interrupted { reason }, _) => format!("interrupted: {reason}"),
                (EndedHow::Failed { error }, _) => format!("could not run: {error}"),
            };
            format!("ended {} ago ({how})", ago(end.at, now))
        }
        (None, Some(exit)) => format!("{}; what it started still runs", status(exit.status)),
        (None, None) => {
            let pid = record.pid.map(|pid| format!(", pid {pid}")).unwrap_or_default();
            format!("running for {}{pid}", ago(record.started_at, now))
        }
    };
    text.push_str(&format!("state    {state}\n"));
    text.push_str(&format!("command  {}\n", first_line(&record.script)));
    text.push_str(&format!("output   {} {}\n", record.machine, shown(&record.output)));
    if let Some(stderr) = &record.stderr {
        text.push_str(&format!("stderr   {} {}\n", record.machine, shown(stderr)));
    }
    text
}

pub(super) fn status(status: ExitStatus) -> String {
    match status {
        ExitStatus::Code(code) => format!("exited {code}"),
        ExitStatus::Signal(signal) => format!("ended by signal {signal}"),
    }
}
