//! `fastctx w`: who is in a town and what each is doing, on every machine it
//! spans; outside every town, the World as a whole; with ids, one resident
//! or job in detail (`world-town.md`).

use std::collections::BTreeSet;

use clap::Args;
use fastctx_hands::told::{duration, turn_end};
use fastctx_types::residents::{Action, Activity, Host, MessageKind, Now, Origin};
use fastctx_types::run::{EndedHow, ExitStatus, JobRecord};
use fastctx_types::town::{Looked, LookedUp, MachineWho, ResidentDetail, ResidentNow, RunningJob, WhoScope, WorldWho};
use fastctx_types::{Address, Call, JobId, Reply, ResidentId, TownName};
use jiff::Timestamp;

/// See who is here and what each is doing.
#[derive(Args)]
pub struct WArgs {
    /// Also list residents who are not here: those who left or moved away,
    /// and colleagues asleep, who can be woken again.
    #[arg(short = 'a', long = "all")]
    all: bool,
    /// A town to look at instead of your own; or residents or jobs to see in
    /// detail, by id, such as codex-7f3a or j-k3x9a2.
    names: Vec<String>,
}

pub fn run(args: WArgs) -> anyhow::Result<()> {
    let you = std::env::var("FASTCTX_RESIDENT").ok().and_then(|address| address.parse::<Address>().ok());
    let now = Timestamp::now();
    let is_id = |name: &str| ResidentId::try_from(name).is_ok() || JobId::try_from(name).is_ok();
    let call = match args.names.as_slice() {
        [] => Call::Who { all: args.all, town: crate::ask::town_from_run() },
        [town] if !is_id(town) => Call::Who { all: args.all, town: Some(TownName::try_from(town.as_str())?) },
        ids => {
            if let Some(name) = ids.iter().find(|name| !is_id(name)) {
                anyhow::bail!("{name} is not a resident's or a job's id; a town is looked at on its own");
            }
            Call::Look { ids: ids.to_vec() }
        }
    };
    let text = match crate::ask::call(call)?.0 {
        Reply::Who(reply) => render(&reply, you.as_ref(), now),
        Reply::Looked { looked } => looked.iter().map(|one| detail(one, now)).collect::<String>(),
        other => anyhow::bail!("the node answered something else: {other:?}"),
    };
    print!("{text}");
    Ok(())
}

fn render(reply: &WorldWho, you: Option<&Address>, now: Timestamp) -> String {
    let mut residents: Vec<&ResidentNow> = Vec::new();
    let mut jobs: Vec<&RunningJob> = Vec::new();
    let mut answered = Vec::new();
    let mut missing = Vec::new();
    for machine in &reply.machines {
        match machine {
            MachineWho::Answered { reply } => {
                answered.push(reply.machine.clone());
                residents.extend(&reply.residents);
                jobs.extend(&reply.jobs);
            }
            MachineWho::Offline { machine, since } => missing.push(match since {
                Some(since) => format!("{machine} offline since {}", clock(*since, now)),
                None => format!("{machine} offline"),
            }),
            MachineWho::NoAnswer { machine, reason } => missing.push(format!("{machine} did not answer ({reason})")),
        }
    }
    // The caller first, then whoever acted most recently.
    residents.sort_by_key(|resident| (Some(&address(resident)) != you, std::cmp::Reverse(latest(resident))));
    let present: BTreeSet<Address> =
        residents.iter().filter(|resident| resident.here).map(|resident| address(resident)).collect();

    let mut machines = Vec::new();
    if reply.machines.len() > 1 {
        machines.push(format!("{} online", answered.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    machines.extend(missing);
    let mut text = match &reply.scope {
        WhoScope::Town { town, record } => {
            let mut facts = vec![counts(&residents)];
            if record.is_none() {
                facts.push(format!("{} keeps no record of {town}, so every machine was asked", reply.machine));
            }
            facts.extend(machines);
            format!("=== w: {town} ({}) ===\n", facts.join("; "))
        }
        WhoScope::World { towns } => {
            // The World as a whole: each town in a line, then those who
            // live in none.
            let mut names: BTreeSet<TownName> = towns.iter().map(|town| town.town.clone()).collect();
            names.extend(residents.iter().filter_map(|resident| resident.town.clone()));
            let mut facts = vec![match names.len() {
                1 => "1 town".to_string(),
                count => format!("{count} towns"),
            }];
            facts.extend(machines);
            let mut text = format!("=== w: the World ({}) ===\n", facts.join("; "));
            if !names.is_empty() {
                text.push_str("towns\n");
                let width = names.iter().map(|name| name.len()).max().unwrap_or(0);
                for name in &names {
                    let living: Vec<&ResidentNow> =
                        residents.iter().copied().filter(|resident| resident.town.as_ref() == Some(name)).collect();
                    let title = towns
                        .iter()
                        .find(|town| town.town == *name)
                        .map(|town| format!("{} · ", town.record.name))
                        .unwrap_or_default();
                    text.push_str(&format!("    {name:width$}  {title}{}\n", counts(&living)));
                }
            }
            residents.retain(|resident| resident.town.is_none());
            text.push_str(&format!("outside every town ({})\n", counts(&residents)));
            text
        }
    };

    let width = residents.iter().map(|resident| resident.about.id.len()).max().unwrap_or(0);
    for resident in &residents {
        let you_mark = if Some(&address(resident)) == you { "  (you)" } else { "" };
        text.push_str(&format!("{:width$}{you_mark}  {}\n", resident.about.id, who(resident, now)));
        let own: Vec<&RunningJob> = jobs.iter().copied().filter(|job| job.owner == address(resident)).collect();
        text.push_str(&format!("    {}\n", doing(resident, own.last().copied(), now)));
    }

    // What runs on with nobody here who started it.
    let unattended: Vec<&&RunningJob> = jobs.iter().filter(|job| !present.contains(&job.owner)).collect();
    if !unattended.is_empty() {
        text.push_str("unattended\n");
        for job in unattended {
            let owner = if answered.contains(&job.owner.machine) {
                format!("started by {}", job.owner.resident)
            } else {
                format!("started by {} on {}, which did not answer", job.owner.resident, job.owner.machine)
            };
            text.push_str(&format!(
                "    {}  {}  {}  {}   {owner}\n",
                job.job,
                job.machine,
                ago(job.started_at, now),
                job.command
            ));
        }
    }
    text
}

/// The first line of a resident: who it is, where, how it came, how long
/// it has been here.
fn who(resident: &ResidentNow, now: Timestamp) -> String {
    let about = &resident.about;
    let mut parts = vec![host(&about.host)];
    if let Some(model) = &about.model {
        parts.push(model.clone());
    }
    parts.push(about.machine.to_string());
    parts.push(match &about.origin {
        Origin::User => "opened by user".to_string(),
        Origin::Colleague { asked_by, label } if asked_by.machine == about.machine => {
            format!("for {} \"{label}\"", asked_by.resident)
        }
        Origin::Colleague { asked_by, label } => format!("for {asked_by} \"{label}\""),
    });
    parts.push(match (&resident.moved, activity(resident)) {
        (Some(moved), _) => match &moved.to {
            Some(town) => format!("moved to {town} {} ago", ago(moved.at, now)),
            None => format!("moved out {} ago", ago(moved.at, now)),
        },
        (None, Some(Activity::Asleep)) => "asleep".to_string(),
        (None, _) if resident.here => format!("here {}", ago(about.born_at, now)),
        (None, _) => "left".to_string(),
    });
    parts.join(" · ")
}

/// The second line: its state, as its host reports it when it does, and
/// what it is doing or did last.
fn doing(resident: &ResidentNow, job: Option<&RunningJob>, now: Timestamp) -> String {
    let mut parts = Vec::new();
    if let Some(state) = &resident.now {
        parts.push(state_of(state, now));
        if state.desk {
            parts.push(format!("using Computer Use on {}", resident.about.machine));
        }
    }
    parts.push(match (job, &resident.last_action) {
        (Some(job), _) => running(job, now),
        (None, Some(action)) => last(action, now),
        (None, None) => "no actions yet".to_string(),
    });
    if let Some(trouble) = resident.now.as_ref().and_then(|state| state.trouble.as_ref()) {
        parts.push(format!("{} ({} ago)", trouble.text, ago(trouble.at, now)));
    }
    parts.join(" · ")
}

/// How many are here, and how many of them busy.
fn counts(residents: &[&ResidentNow]) -> String {
    let here = residents.iter().filter(|resident| resident.here).count();
    let busy = residents.iter().filter(|resident| activity(resident) == Some(Activity::Busy)).count();
    if busy > 0 { format!("{here} here, {busy} busy") } else { format!("{here} here") }
}

fn state_of(state: &Now, now: Timestamp) -> String {
    match state.activity {
        Activity::Busy => format!("busy {}", ago(state.since, now)),
        Activity::Idle => format!("idle {}", ago(state.since, now)),
        Activity::Asleep => format!("asleep {}", ago(state.since, now)),
    }
}

/// One looked-up id in detail.
fn detail(looked: &LookedUp, now: Timestamp) -> String {
    let mut text = String::new();
    if looked.found.is_empty() {
        text.push_str(&format!("=== w {} (no machine that answered has it) ===\n", looked.id));
    }
    for found in &looked.found {
        match found {
            Looked::Resident(detail) => text.push_str(&resident_detail(detail, now)),
            Looked::Job { record } => text.push_str(&job_detail(record, now)),
        }
    }
    if !looked.unasked.is_empty() {
        text.push_str(&format!("=== not asked: {} ===\n", looked.unasked.join(", ")));
    }
    text
}

fn resident_detail(detail: &ResidentDetail, now: Timestamp) -> String {
    let resident = &detail.resident;
    let about = &resident.about;
    let machine = &about.machine;
    let mut text = format!("=== w {}: {} ===\n", about.id, who(resident, now));
    if let Origin::Colleague { label, .. } = &about.origin {
        text.push_str(&format!("task     \"{label}\"   (the full request is in its records)\n"));
    }
    let job = detail.jobs.last();
    text.push_str(&format!("state    {}\n", doing(resident, job, now)));
    if let Some(job) = job {
        text.push_str(&format!("         output so far: {machine} {}\n", job.output.replace('\\', "/")));
    }
    if detail.actions_total == 0 {
        text.push_str("actions  none yet\n");
    } else {
        text.push_str(&format!(
            "actions  last {} of {} (all of them: {machine} {}/actions)\n",
            detail.actions.len(),
            detail.actions_total,
            detail.records
        ));
        for action in &detail.actions {
            text.push_str(&format!(
                "  {} {}  {}  {} -> {}\n",
                clock(action.at, now),
                action.machine,
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
            text.push_str(&format!(
                "answers  latest {} (all of them: {machine} {}/messages)\n",
                detail.answers.len(),
                detail.records
            ));
            for line in &detail.answers {
                if let MessageKind::Ended { end, .. } = &line.what {
                    let message = match end.last_message.as_deref().map(first_line) {
                        Some(message) if !message.is_empty() => format!(": {message}"),
                        _ => String::new(),
                    };
                    text.push_str(&format!("  {} {}{message}\n", clock(line.at, now), turn_end(end)));
                }
            }
        }
    }
    text
}

fn job_detail(record: &JobRecord, now: Timestamp) -> String {
    let mut text = format!(
        "=== w {}: on {}, started by {} {} ago ===\n",
        record.id,
        record.machine,
        record.owner,
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
    text.push_str(&format!("output   {} {}\n", record.machine, record.output.replace('\\', "/")));
    text
}

fn status(status: ExitStatus) -> String {
    match status {
        ExitStatus::Code(code) => format!("exited {code}"),
        ExitStatus::Signal(signal) => format!("ended by signal {signal}"),
    }
}

fn address(resident: &ResidentNow) -> Address {
    Address { machine: resident.about.machine.clone(), resident: resident.about.id.clone() }
}

fn activity(resident: &ResidentNow) -> Option<Activity> {
    resident.now.as_ref().map(|now| now.activity)
}

/// When it last did anything the World saw.
fn latest(resident: &ResidentNow) -> Timestamp {
    let action = resident.last_action.as_ref().map(|action| action.at);
    let state = resident.now.as_ref().map(|now| now.since);
    action.into_iter().chain(state).max().unwrap_or(resident.about.born_at)
}

fn host(host: &Host) -> String {
    let name = match host.agent.as_str() {
        "codex" => "Codex".to_string(),
        "claude" => "Claude Code".to_string(),
        _ => host.client.clone(),
    };
    if host.version.is_empty() { name } else { format!("{name} {}", host.version) }
}

fn running(job: &RunningJob, now: Timestamp) -> String {
    let pid = job.pid.map(|pid| format!(", pid {pid}")).unwrap_or_default();
    format!("running on {} for {} ({}{pid}): {}", job.machine, ago(job.started_at, now), job.job, job.command)
}

fn last(action: &Action, now: Timestamp) -> String {
    format!("{} {} ago on {}: {} -> {}", action.hand, ago(action.at, now), action.machine, action.what, action.result)
}

fn first_line(text: &str) -> String {
    let mut lines = text.trim().lines();
    let first = lines.next().unwrap_or_default().to_string();
    if lines.next().is_some() { format!("{first} …") } else { first }
}

/// How long since `then`, as `45s`, `3m`, `2h10m` or `4d2h`.
fn ago(then: Timestamp, now: Timestamp) -> String {
    duration(now.as_second().saturating_sub(then.as_second()).max(0) as u64)
}

/// A moment as the local clock reads it: the time today, the date before.
fn clock(at: Timestamp, now: Timestamp) -> String {
    let zone = jiff::tz::TimeZone::system();
    let (at, now) = (at.to_zoned(zone.clone()), now.to_zoned(zone));
    if at.date() == now.date() { at.strftime("%H:%M").to_string() } else { at.strftime("%Y-%m-%d %H:%M").to_string() }
}
