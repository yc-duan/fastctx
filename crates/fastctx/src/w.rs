//! `fastctx w`: who is in a town and what each is doing, on every machine it
//! spans; outside every town, the World as a whole; with ids, one resident
//! or job in detail (`world-town.md`).

use clap::Args;
use fastctx_client::town::{MachineState, Scope, address};
use fastctx_client::{TownView, World};
use fastctx_hands::told::{duration, turn_end};
use fastctx_types::residents::{Action, Activity, Host, MessageKind, Now, Origin};
use fastctx_types::run::{EndedHow, ExitStatus, JobRecord};
use fastctx_types::town::{Looked, LookedUp, ResidentDetail, ResidentNow, RunningJob};
use fastctx_types::{Address, JobId, MachineName, ResidentId, TownName};
use jiff::Timestamp;

/// See who is here and what each is doing.
#[derive(Args)]
pub struct WArgs {
    /// Also list residents who are not here: those who left or moved away,
    /// and colleagues asleep, who can be woken again.
    #[arg(short = 'a', long = "all")]
    all: bool,
    /// A town to look at instead of your own; or residents or jobs to see in
    /// detail, by id, such as codex-7f3a or j-k3x9a2, or codex-7f3a@laptop for
    /// the one on that machine.
    names: Vec<String>,
}

pub fn run(args: WArgs) -> anyhow::Result<()> {
    let you = std::env::var("FASTCTX_RESIDENT").ok().and_then(|address| address.parse::<Address>().ok());
    let now = Timestamp::now();
    let is_id = |name: &str| {
        name.parse::<Address>().is_ok() || ResidentId::try_from(name).is_ok() || JobId::try_from(name).is_ok()
    };
    let world = World::here()?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let text = match args.names.as_slice() {
        [] => {
            let who = runtime.block_on(world.who(crate::ask::town_from_run(), args.all))?;
            render(&TownView::new(who, you.as_ref()), you.as_ref(), now)
        }
        [town] if !is_id(town) => {
            let who = runtime.block_on(world.who(Some(TownName::try_from(town.as_str())?), args.all))?;
            render(&TownView::new(who, you.as_ref()), you.as_ref(), now)
        }
        ids => {
            if let Some(name) = ids.iter().find(|name| !is_id(name)) {
                anyhow::bail!("{name} is not a resident's or a job's id; a town is looked at on its own");
            }
            // An id at a machine is asked for everywhere, as any id is, and
            // only that machine's answer is kept.
            let at: Vec<Option<MachineName>> =
                ids.iter().map(|name| name.parse::<Address>().ok().map(|address| address.machine)).collect();
            let bare = ids.iter().map(|name| name.split_once('@').map_or(name.as_str(), |(id, _)| id).to_string());
            let mut looked = runtime.block_on(world.look(bare.collect()))?;
            for (one, machine) in looked.iter_mut().zip(&at) {
                if let Some(machine) = machine {
                    one.found.retain(|found| match found {
                        Looked::Resident(detail) => &detail.resident.about.machine == machine,
                        Looked::Job { record } => &record.machine == machine,
                    });
                    one.unasked.retain(|unasked| unasked == machine.as_str());
                }
            }
            looked.iter().map(|one| detail(one, now)).collect::<String>()
        }
    };
    print!("{text}");
    Ok(())
}

fn render(view: &TownView, you: Option<&Address>, now: Timestamp) -> String {
    let mut machines = Vec::new();
    let answered: Vec<&str> = view
        .machines
        .iter()
        .filter_map(|state| match state {
            MachineState::Answered(machine) => Some(machine.as_str()),
            _ => None,
        })
        .collect();
    if view.machines.len() > 1 {
        machines.push(format!("{} online", answered.join(", ")));
    }
    for state in &view.machines {
        match state {
            MachineState::Answered(_) => {}
            MachineState::Offline { machine, since: Some(since) } => {
                machines.push(format!("{machine} offline since {}", clock(*since, now)))
            }
            MachineState::Offline { machine, since: None } => machines.push(format!("{machine} offline")),
            MachineState::NoAnswer { machine, reason } => machines.push(format!("{machine} did not answer ({reason})")),
        }
    }
    let mut text = match &view.scope {
        Scope::Town { town, record } => {
            let mut facts = vec![counts(view.counts())];
            if record.is_none() {
                facts.push(format!("{} keeps no record of {town}, so every machine was asked", view.machine));
            }
            facts.extend(machines);
            format!("=== w: {town} ({}) ===\n", facts.join("; "))
        }
        Scope::World { towns } => {
            // The World as a whole: each town in a line, then those who
            // live in none.
            let mut facts = vec![match towns.len() {
                1 => "1 town".to_string(),
                count => format!("{count} towns"),
            }];
            facts.extend(machines);
            let mut text = format!("=== w: the World ({}) ===\n", facts.join("; "));
            if !towns.is_empty() {
                text.push_str("towns\n");
                let width = towns.iter().map(|town| town.town.len()).max().unwrap_or(0);
                for town in towns {
                    let title =
                        town.summary.as_ref().map(|summary| format!("{} · ", summary.record.name)).unwrap_or_default();
                    text.push_str(&format!("    {:width$}  {title}{}\n", town.town, counts((town.here, town.busy))));
                }
            }
            text.push_str(&format!("outside every town ({})\n", counts(view.counts())));
            text
        }
    };

    let width = view.residents.iter().map(|resident| resident.about.id.len()).max().unwrap_or(0);
    for resident in &view.residents {
        let you_mark = if Some(&address(resident)) == you { "  (you)" } else { "" };
        text.push_str(&format!("{:width$}{you_mark}  {}\n", resident.about.id, who(resident, now)));
        let own = view.jobs_of(resident);
        text.push_str(&format!("    {}\n", doing(resident, own.last().copied(), now)));
    }

    // What runs on with nobody here who started it.
    let unattended = view.unattended();
    if !unattended.is_empty() {
        text.push_str("unattended\n");
        for job in unattended {
            let owner = if view.answered(&job.owner.machine) {
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
fn counts((here, busy): (usize, usize)) -> String {
    if busy > 0 { format!("{here} here, {busy} busy") } else { format!("{here} here") }
}

fn state_of(state: &Now, now: Timestamp) -> String {
    match state.activity {
        Activity::Busy if state.waiting_for_user => {
            format!("waiting for the user's answer · turn running {}", ago(state.since, now))
        }
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

fn activity(resident: &ResidentNow) -> Option<Activity> {
    resident.now.as_ref().map(|now| now.activity)
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
