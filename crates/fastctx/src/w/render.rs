use super::*;
use fastctx_types::town::CurrentTool;

pub(super) fn overview(view: &TownView, you: Option<&Address>, all: bool, now: Timestamp, local_only: bool, sentinels: &[fastctx_types::sentinel_control::Summary]) -> String {
    let mut machines = Vec::new();
    let online = view
        .machines
        .iter()
        .filter_map(|state| match state {
            MachineState::Answered(machine) => Some(machine.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !online.is_empty() {
        machines.push(format!(
            "{} {}",
            online.join(", "),
            if local_only { "local records; node not running" } else { "online" }
        ));
    }
    // Machines that did not answer for one reason are said together: a hub
    // this computer cannot reach leaves every machine on it unasked.
    let mut silent: Vec<(&str, Vec<String>)> = Vec::new();
    for state in &view.machines {
        match state {
            MachineState::Answered(_) => {}
            MachineState::Offline { machine, since: Some(since) } => {
                machines.push(format!("{machine} offline for {}", ago(*since, now)))
            }
            MachineState::Offline { machine, since: None } => machines.push(format!("{machine} offline")),
            MachineState::NoAnswer { machine, reason } => {
                match silent.iter_mut().find(|(said, _)| *said == reason.as_str()) {
                    Some((_, names)) => names.push(machine.to_string()),
                    None => silent.push((reason, vec![machine.to_string()])),
                }
            }
        }
    }
    machines
        .extend(silent.into_iter().map(|(reason, names)| format!("{} did not answer ({reason})", names.join(", "))));
    machines.extend(view.unanswered.iter().map(|missing| format!("not answered: {missing}")));
    let place = match &view.scope {
        Scope::Town { town, .. } => town.as_str(),
        Scope::World { .. } => "all projects",
    };
    let mut text = format!("=== w: {place} ({}) ===\n", machines.join("; "));
    if let Scope::Town { town, record: None } = &view.scope {
        text.push_str(&format!("{} has no record of {town}; every reachable machine was asked.\n", view.machine));
    }
    if let Scope::World { towns } = &view.scope {
        if !towns.is_empty() {
            text.push_str("projects\n");
        }
        for town in towns {
            let description = town
                .summary
                .as_ref()
                .map(|summary| {
                    format!(
                        "  {}  ({})",
                        summary.record.name,
                        summary.record.machines.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
                    )
                })
                .unwrap_or_default();
            let activity = if local_only {
                "current activity unverified".into()
            } else {
                format!("{} working, {} running", town.busy, town.running)
            };
            text.push_str(&format!("  {}{description}  {activity}\n", town.town));
        }
    }
    let this = std::env::var("FASTCTX_JOB").ok();
    let mut running = Vec::new();
    for job in &view.jobs {
        if this.as_deref() == Some(job.job.as_str()) && job.machine == view.machine {
            continue;
        }
        let owner = match job.owner.resident() {
            Some(owner) => {
                let absence = match view.known(owner) {
                    Some(r) if !r.here => {
                        if let Some(at) = r.removed {
                            format!(" (removed {} ago)", ago(at, now))
                        } else if let Some(moved) = &r.moved {
                            format!(
                                " (moved to {} {} ago)",
                                moved.to.as_ref().map_or("outside", |t| t.as_str()),
                                ago(moved.at, now)
                            )
                        } else {
                            " (left)".into()
                        }
                    }
                    None if view.shows(&owner.machine) && !view.answered(&owner.machine) => {
                        " (owner's machine did not answer)".into()
                    }
                    _ => String::new(),
                };
                // A machine the caller cannot use is not named: an assisting
                // agent's conversation lives on its owner's computer.
                if view.shows(&owner.machine) {
                    format!("{}{absence}", person(owner, &job.machine))
                } else {
                    format!("{}{absence}", owner.resident)
                }
            }
            None => job.owner.to_string(),
        };
        let owner = match view.works_for_in(&job.machine, job.job.as_str(), job.source.as_ref()) {
            Some(accounts) => format!("{owner} for {accounts}"),
            None => owner,
        };
        running.push((
            job.machine.clone(),
            format!(
                "  {}  {}  {}  {owner}  {}\n",
                job.written(),
                job.job,
                ago(job.started_at, now),
                brief(&job.command)
            ),
        ));
    }
    for resident in &view.residents {
        if let Some(state) = &resident.now
            && resident.here
            && state.desk
        {
            let time = state
                .desk_since
                .map(|at| format!(">={}", ago(at, now)))
                .unwrap_or_else(|| "duration unrecorded".into());
            running.push((
                resident.about.machine.clone(),
                format!("  {}  desk  {time}  {}  Computer Use\n", resident.written(), resident.about.id),
            ));
        }
    }
    for sentinel in sentinels {
        if matches!(view.scope, Scope::Town { .. }) && view.known(&sentinel.definition.owner).is_none() { continue; }
        running.push((sentinel.definition.home.clone(), fastctx_types::sentinel_words::row(sentinel)));
    }
    running.sort_by(|a, b| a.0.cmp(&b.0));
    if !running.is_empty() {
        text.push_str("running\n");
    }
    for (_, line) in running {
        text.push_str(&line);
    }
    let mut working = String::new();
    let mut waiting = String::new();
    let mut resting = String::new();
    let mut left = 0;
    for resident in &view.residents {
        if busy(resident) || (resident.here && resident.now.is_none()) {
            let mark = if Some(&address(resident)) == you { " (you)" } else { "" };
            let whom = view
                .works_for_in(&resident.about.machine, resident.about.id.as_str(), resident.source.as_ref())
                .map(|accounts| format!(" for {accounts}"))
                .unwrap_or_default();
            let line = format!(
                "  {}{mark}  {}  {}{whom}{}  {}",
                resident.about.id,
                resident.written(),
                host(&resident.about.host),
                task(resident),
                brief(&step(resident, now))
            );
            working.push_str(line.trim_end());
            working.push('\n');
        } else if all && resident.now.is_none() && resident.removed.is_none() && resident.moved.is_none() {
            // A session that has left cannot be woken or spoken to again;
            // what it did is in the history, and `w ID` finds it.
            left += 1;
        } else if all {
            let state = if let Some(at) = resident.removed {
                format!("removed {} ago", ago(at, now))
            } else if let Some(moved) = &resident.moved {
                format!("moved to {} {} ago", moved.to.as_ref().map_or("outside", |t| t.as_str()), ago(moved.at, now))
            } else if let Some(state) = &resident.now {
                match state.activity {
                    Activity::Busy => brief(&step(resident, now)),
                    Activity::Idle if !resident.here => "session not open; last reported idle".into(),
                    Activity::Idle => "idle".into(),
                    // Older now records did not consistently timestamp sleep.
                    Activity::Asleep => "asleep".into(),
                }
            } else {
                unreachable!("residents that left are counted, not listed")
            };
            let last = resident
                .last_action
                .as_ref()
                .map(|action| {
                    format!(
                        "; {} {} ago: {} -> {}",
                        action.hand,
                        ago(action.at, now),
                        brief(&action.what),
                        action.result
                    )
                })
                .unwrap_or_default();
            resting.push_str(&format!(
                "  {}  {}  {}{}  {state}{last}\n",
                resident.about.id,
                resident.written(),
                host(&resident.about.host),
                task(resident)
            ));
        }
        if let Some(state) = &resident.now {
            for (turn, readers) in &state.owed {
                for reader in readers {
                    let time = if state.turn.as_ref() == Some(turn) {
                        format!(", turn {}", ago(state.since, now))
                    } else {
                        String::new()
                    };
                    waiting.push_str(&format!(
                        "  {} on {} owes {} an answer{time}\n",
                        resident.about.id,
                        resident.written(),
                        person(reader, &resident.about.machine)
                    ));
                }
            }
            if resident.here && state.waiting_for_user {
                let time = state.waiting_since.map(|at| format!(", {}", ago(at, now))).unwrap_or_default();
                waiting.push_str(&format!(
                    "  {} on {} waits for the user to answer{time}\n",
                    resident.about.id,
                    resident.written()
                ));
            }
        }
    }
    if left > 0 {
        resting.push_str(&format!("  and {left} sessions that have left; `fastctx w ID` shows one\n"));
    }
    for (label, body) in [("working", working), ("waiting", waiting), ("inactive", resting)] {
        if !body.is_empty() {
            text.push_str(&format!("{label}\n{body}"));
        }
    }
    text
}

/// A command or a resident's current step as one line of a list: its first
/// line, no longer than a terminal line, marked where cut; `w ID` gives it
/// whole.
pub(super) fn brief(command: &str) -> String {
    const WIDTH: usize = 100;
    let first = first_line(command);
    if first.chars().count() <= WIDTH {
        first
    } else {
        format!("{} …", first.chars().take(WIDTH).collect::<String>())
    }
}

pub(super) fn person(address: &Address, machine: &MachineName) -> String {
    if &address.machine == machine { address.resident.to_string() } else { address.to_string() }
}

pub(super) fn task(resident: &ResidentNow) -> String {
    match &resident.about.origin {
        Origin::Colleague { asked_by, label } => {
            format!("  for {} \"{label}\"", person(asked_by, &resident.about.machine))
        }
        Origin::User => resident
            .now
            .as_ref()
            .and_then(|state| state.title.as_ref())
            .map(|title| format!("  \"{title}\""))
            .unwrap_or_default(),
    }
}

pub(super) fn step(resident: &ResidentNow, now: Timestamp) -> String {
    match &resident.now {
        Some(state) if state.activity == Activity::Busy => {
            if !resident.here {
                return format!("session not open; last recorded turn began {} ago", ago(state.since, now));
            }
            let tools = resident
                .tools
                .iter()
                .filter(|tool| state.turn.as_ref() == Some(&tool.turn))
                .map(tool)
                .collect::<Vec<_>>();
            let doing = if tools.is_empty() { String::new() } else { format!(": {}", tools.join("; ")) };
            if state.turn.is_some() {
                format!("turn {}{doing}", ago(state.since, now))
            } else {
                format!("turn not yet identified{doing}")
            }
        }
        Some(state) => match state.activity {
            Activity::Idle => "idle".into(),
            Activity::Asleep => "asleep".into(),
            Activity::Busy => unreachable!(),
        },
        None => resident
            .last_action
            .as_ref()
            .map(|action| format!("last fastctx action {} ago: {} {}", ago(action.at, now), action.hand, action.what))
            .unwrap_or_default(),
    }
}

pub(super) fn tool(tool: &CurrentTool) -> String {
    if tool.detail.is_empty() { tool.name.clone() } else { format!("{} {}", tool.name, tool.detail) }
}
