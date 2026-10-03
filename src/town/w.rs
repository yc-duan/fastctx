//! Renders `fastctx w` from the machines' answers (`specs/world-town.md`): the town overview,
//! the World level for a resident outside every town, and the detail of residents and jobs.
//! Every word comes from the records, the hosts' reports, or a requester's label.

use super::records::Birth;
use super::who::{JobFacts, MachineAnswer, Query, ResidentFacts, is_here};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// A machine of the World that gave no answer, and why.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct Silent {
    pub(crate) machine: String,
    pub(crate) why: Silence,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub(crate) enum Silence {
    /// Offline since the hub last saw it, when known.
    Offline(Option<String>),
    /// Online, but the call failed or got no answer.
    NoAnswer(String),
}

/// Everything one `fastctx w` shows.
pub(crate) struct View<'a> {
    pub(crate) caller: Option<&'a str>,
    pub(crate) query: &'a Query,
    pub(crate) answers: &'a [MachineAnswer],
    pub(crate) silent: &'a [Silent],
}

pub(crate) fn render(view: &View<'_>, now: OffsetDateTime) -> String {
    if !view.query.ids.is_empty() {
        return render_details(view, now);
    }
    match &view.query.town {
        Some(town) => render_town(view, town, now),
        None => render_world(view, now),
    }
}

fn render_town(view: &View<'_>, town: &str, now: OffsetDateTime) -> String {
    let residents = sorted_residents(view);
    let jobs: Vec<&JobFacts> = view
        .answers
        .iter()
        .flat_map(|answer| &answer.jobs)
        .collect();
    let here = residents
        .iter()
        .filter(|facts| is_here(&facts.now.state))
        .count();
    let busy = residents
        .iter()
        .filter(|facts| facts.now.state == "busy")
        .count();
    let mut out = format!(
        "=== w: {town} ({here} here, {busy} busy; {}) ===\n",
        reach(view)
    );
    write_residents(&mut out, view.caller, &residents, &jobs, now);
    write_unattended(&mut out, &residents, &jobs, now);
    out
}

fn render_world(view: &View<'_>, now: OffsetDateTime) -> String {
    let mut towns: BTreeMap<&str, (usize, usize, Vec<String>)> = BTreeMap::new();
    for answer in view.answers {
        for count in &answer.towns {
            let entry = towns.entry(count.town.as_str()).or_default();
            entry.0 += count.here;
            entry.1 += count.busy;
            if count.here > 0 {
                entry.2.push(format!("{} {}", answer.machine, count.here));
            }
        }
    }
    let residents = sorted_residents(view);
    let jobs: Vec<&JobFacts> = view
        .answers
        .iter()
        .flat_map(|answer| &answer.jobs)
        .collect();
    let mut out = format!(
        "=== w: the World ({} with residents here, {} outside every town; {}) ===\n",
        plural(
            towns.values().filter(|(here, _, _)| *here > 0).count(),
            "town"
        ),
        residents
            .iter()
            .filter(|facts| is_here(&facts.now.state))
            .count(),
        reach(view)
    );
    if !towns.is_empty() {
        out.push_str("towns\n");
        let width = towns.keys().map(|name| name.len()).max().unwrap_or(0);
        for (name, (here, busy, machines)) in &towns {
            let spread = if machines.is_empty() {
                String::new()
            } else {
                format!("   ({})", machines.join(", "))
            };
            let _ = writeln!(out, "    {name:<width$}  {here} here, {busy} busy{spread}");
        }
    }
    if !residents.is_empty() {
        out.push_str("outside\n");
        let mut block = String::new();
        write_residents(&mut block, view.caller, &residents, &jobs, now);
        for line in block.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
    write_unattended(&mut out, &residents, &jobs, now);
    out
}

/// "desktop, laptop online; hangzhou offline since 13:40Z": which machines this view covers.
fn reach(view: &View<'_>) -> String {
    let mut parts = Vec::new();
    let answered: Vec<&str> = view
        .answers
        .iter()
        .map(|answer| answer.machine.as_str())
        .collect();
    if !answered.is_empty() {
        parts.push(format!("{} online", answered.join(", ")));
    }
    for silent in view.silent {
        parts.push(match &silent.why {
            Silence::Offline(Some(since)) => {
                format!("{} offline since {}", silent.machine, clock(since))
            }
            Silence::Offline(None) => format!("{} offline", silent.machine),
            Silence::NoAnswer(reason) => format!("{} did not answer: {reason}", silent.machine),
        });
    }
    parts.join("; ")
}

/// The caller first, then whoever moved most recently.
fn sorted_residents<'a>(view: &View<'a>) -> Vec<&'a ResidentFacts> {
    let mut residents: Vec<&ResidentFacts> = view
        .answers
        .iter()
        .flat_map(|answer| &answer.residents)
        .collect();
    residents.sort_by(|left, right| {
        let caller = |facts: &ResidentFacts| Some(facts.about.id.as_str()) == view.caller;
        caller(right)
            .cmp(&caller(left))
            .then_with(|| last_moved(right).cmp(last_moved(left)))
    });
    residents
}

fn last_moved(facts: &ResidentFacts) -> &str {
    match &facts.now.last_action_at {
        Some(at) if at.as_str() > facts.now.since.as_str() => at,
        _ => &facts.now.since,
    }
}

fn write_residents(
    out: &mut String,
    caller: Option<&str>,
    residents: &[&ResidentFacts],
    jobs: &[&JobFacts],
    now: OffsetDateTime,
) {
    let width = residents
        .iter()
        .map(|facts| facts.about.id.len())
        .max()
        .unwrap_or(0);
    for facts in residents {
        let you = if Some(facts.about.id.as_str()) == caller {
            "(you)  "
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{:<width$}  {you}{}",
            facts.about.id,
            identity(facts, now)
        );
        let _ = writeln!(out, "    {}", doing(facts, jobs, now));
    }
}

/// "Claude Code · opus-5-5 · desktop VS Code · opened by user · here 2h10m".
fn identity(facts: &ResidentFacts, now: OffsetDateTime) -> String {
    let about = &facts.about;
    let mut parts = vec![agent_name(&about.agent)];
    if let Some(model) = &about.model {
        parts.push(if about.model_source.as_deref() == Some("host") {
            model.clone()
        } else {
            format!("{model} (self-reported)")
        });
    }
    let surface = surface_name(&about.surface);
    parts.push(if surface.is_empty() {
        about.machine.clone()
    } else {
        format!("{} {surface}", about.machine)
    });
    parts.push(match &about.birth {
        Birth::User => "opened by user".to_string(),
        Birth::Colleague {
            requester,
            label,
            machine,
        } => {
            let requester = match machine {
                Some(machine) => format!("{requester} on {machine}"),
                None => requester.clone(),
            };
            if label.is_empty() {
                format!("for {requester}")
            } else {
                format!("for {requester} \"{label}\"")
            }
        }
    });
    if is_here(&facts.now.state) {
        parts.push(format!("here {}", span(&about.since, now)));
    }
    parts.join(" · ")
}

/// The second line: state, then what it is doing or did last.
fn doing(facts: &ResidentFacts, jobs: &[&JobFacts], now: OffsetDateTime) -> String {
    let since = span(&facts.now.since, now);
    let state = match facts.now.state.as_str() {
        "busy" => "busy".to_string(),
        "idle" => format!("idle {since}"),
        "waiting" => format!("waiting for input {since}"),
        "asleep" => format!("asleep {since}"),
        "left" => format!("left {since} ago"),
        "interrupted" => format!("interrupted {since} ago"),
        "error" => format!("failed to start {since} ago"),
        _ => String::new(),
    };
    let mut running: Vec<&&JobFacts> = jobs
        .iter()
        .filter(|job| job.resident == facts.about.id && job.running())
        .collect();
    running.sort_by(|left, right| right.started_at.cmp(&left.started_at));
    let activity = match running.first() {
        Some(job) => {
            let more = if running.len() > 1 {
                format!(" (and {} more running)", running.len() - 1)
            } else {
                String::new()
            };
            format!(
                "running on {} for {}: {}{more}",
                job.machine,
                span(&job.started_at, now),
                first_line(&job.command)
            )
        }
        None => match (&facts.now.last_action, &facts.now.last_action_at) {
            (Some(action), Some(at)) => format!("{action} {} ago", span(at, now)),
            (Some(action), None) => action.clone(),
            _ => "no actions yet".to_string(),
        },
    };
    let desk = if facts.now.desk {
        " · at this machine's Computer Use desk"
    } else {
        ""
    };
    let trouble = facts
        .now
        .trouble
        .as_ref()
        .map(|trouble| format!(" · {} {}", clock(&trouble.at), first_line(&trouble.text)))
        .unwrap_or_default();
    if state.is_empty() {
        format!("{activity}{desk}{trouble}")
    } else {
        format!("{state} · {activity}{desk}{trouble}")
    }
}

/// Running things whose starter is not here.
fn write_unattended(
    out: &mut String,
    residents: &[&ResidentFacts],
    jobs: &[&JobFacts],
    now: OffsetDateTime,
) {
    let starter = |job: &JobFacts| {
        residents
            .iter()
            .find(|facts| facts.about.id == job.resident)
            .copied()
    };
    let mut left: Vec<&&JobFacts> = jobs
        .iter()
        .filter(|job| job.running() && !starter(job).is_some_and(|facts| is_here(&facts.now.state)))
        .collect();
    if left.is_empty() {
        return;
    }
    left.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    out.push_str("unattended\n");
    for job in left {
        let by = match starter(job) {
            Some(facts) if facts.now.state == "left" => format!(
                "started by {}, left {} ago",
                job.resident,
                span(&facts.now.since, now)
            ),
            Some(facts) => format!("started by {}, {}", job.resident, facts.now.state),
            None => format!("started by {}", job.resident),
        };
        let _ = writeln!(
            out,
            "    {}  {}  {}  {}   {by}",
            job.id,
            job.machine,
            span(&job.started_at, now),
            first_line(&job.command)
        );
    }
}

fn render_details(view: &View<'_>, now: OffsetDateTime) -> String {
    let mut out = String::new();
    for id in &view.query.ids {
        let resident = super::who::current(
            view.answers
                .iter()
                .flat_map(|answer| &answer.residents)
                .filter(|facts| &facts.about.id == id)
                .collect(),
        );
        let job = view
            .answers
            .iter()
            .flat_map(|answer| &answer.jobs)
            .find(|job| &job.id == id);
        let jobs: Vec<&JobFacts> = view
            .answers
            .iter()
            .flat_map(|answer| &answer.jobs)
            .collect();
        match (resident, job) {
            (Some(facts), _) => write_resident_detail(&mut out, facts, &jobs, now),
            (None, Some(job)) => write_job_detail(&mut out, job, now),
            (None, None) => {
                let _ = writeln!(
                    out,
                    "=== w {id}: no resident or job with this id ({}) ===",
                    reach(view)
                );
            }
        }
    }
    out
}

fn write_resident_detail(
    out: &mut String,
    facts: &ResidentFacts,
    jobs: &[&JobFacts],
    now: OffsetDateTime,
) {
    let about = &facts.about;
    let _ = writeln!(out, "=== w {}: {} ===", about.id, identity(facts, now));
    if let Birth::Colleague { label, .. } = &about.birth {
        let _ = writeln!(out, "task     \"{label}\"   (full request in its records)");
    }
    let _ = writeln!(out, "state    {}", doing(facts, jobs, now));
    for job in jobs
        .iter()
        .filter(|job| job.resident == about.id && job.running())
    {
        let pid = job
            .pid
            .map(|pid| format!(", pid {pid}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "         {} ({}{pid}) output so far: {} {}",
            job.id,
            job.machine,
            job.machine,
            crate::paths::display_path(&job.output_path)
        );
    }
    let actions = facts.dir.join("actions");
    if facts.actions_total == 0 {
        let _ = writeln!(out, "actions  none yet");
    } else {
        let _ = writeln!(
            out,
            "actions  last {} of {} (all of them: {} {})",
            facts.actions.len(),
            facts.actions_total,
            about.machine,
            crate::paths::display_path(&actions)
        );
        let hand_width = facts
            .actions
            .iter()
            .map(|action| action.hand.len())
            .max()
            .unwrap_or(0);
        for action in &facts.actions {
            let result = if action.result.is_empty() {
                String::new()
            } else {
                format!(" -> {}", action.result)
            };
            let _ = writeln!(
                out,
                "  {} {}  {:<hand_width$}  {}{result}",
                clock(&action.at),
                action.machine,
                action.hand,
                action.what
            );
        }
    }
    if facts.damaged_messages > 0 {
        let _ = writeln!(
            out,
            "damaged  {} lines of {} could not be read",
            facts.damaged_messages,
            crate::paths::display_path(&facts.dir.join("messages"))
        );
    }
    let answers: Vec<_> = facts
        .messages
        .iter()
        .filter(|line| line.kind == "answered")
        .collect();
    match answers.last() {
        None => {
            let _ = writeln!(out, "answers  none yet");
        }
        Some(last) => {
            let readers = if last.peer.is_empty() {
                String::new()
            } else {
                format!(" to {}", last.peer)
            };
            let _ = writeln!(
                out,
                "answers  {} (last at {}{readers}; every word in {} {})",
                answers.len(),
                clock(&last.at),
                about.machine,
                crate::paths::display_path(&facts.dir.join("messages"))
            );
        }
    }
}

fn write_job_detail(out: &mut String, job: &JobFacts, now: OffsetDateTime) {
    let pid = job
        .pid
        .map(|pid| format!(", pid {pid}"))
        .unwrap_or_default();
    let head = match (job.state.as_str(), job.exit_code, &job.ended_at) {
        ("starting" | "running", _, _) => format!(
            "running on {} for {}{pid}",
            job.machine,
            span(&job.started_at, now)
        ),
        (state, code, Some(ended)) => {
            let code = code.map(|code| format!(" {code}")).unwrap_or_default();
            format!(
                "{state}{code} on {} after {}, {} ago",
                job.machine,
                between(&job.started_at, ended),
                span(ended, now)
            )
        }
        (state, _, None) => format!("{state} on {}", job.machine),
    };
    let _ = writeln!(out, "=== w {}: {head} ===", job.id);
    let _ = writeln!(out, "command  {}", job.command);
    let _ = writeln!(
        out,
        "started  by {} at {}",
        job.resident,
        clock(&job.started_at)
    );
    let _ = writeln!(
        out,
        "output   {} {}",
        job.machine,
        crate::paths::display_path(&job.output_path)
    );
}

fn agent_name(agent: &str) -> String {
    match agent {
        "claude-code" => "Claude Code".to_string(),
        "codex" | "codex-mcp-client" => "Codex".to_string(),
        other => other.to_string(),
    }
}

/// Where the host runs it, in the words a reader expects; the World's own launches say
/// nothing here because the birth already tells.
fn surface_name(surface: &str) -> String {
    match surface {
        "claude-vscode" | "vscode" => "VS Code".to_string(),
        "claude-desktop" | "app" => "app".to_string(),
        "cli" => "CLI".to_string(),
        "bg" => "background".to_string(),
        "world" | "unknown" | "" => String::new(),
        other => other.to_string(),
    }
}

fn first_line(command: &str) -> String {
    let mut lines = command.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().unwrap_or_default().trim().to_string();
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first
    }
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// How long ago an RFC 3339 moment was, as "40s", "12m", "2h10m" or "3d4h".
fn span(at: &str, now: OffsetDateTime) -> String {
    match OffsetDateTime::parse(at, &Rfc3339) {
        Ok(then) => duration((now - then).whole_seconds()),
        Err(_) => "?".to_string(),
    }
}

fn between(from: &str, to: &str) -> String {
    match (
        OffsetDateTime::parse(from, &Rfc3339),
        OffsetDateTime::parse(to, &Rfc3339),
    ) {
        (Ok(from), Ok(to)) => duration((to - from).whole_seconds()),
        _ => "?".to_string(),
    }
}

fn duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h{}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d{}h", seconds / 86_400, seconds % 86_400 / 3600),
    }
}

/// "14:21Z" today, "10-01 14:21Z" on another day (UTC, as every record is written).
fn clock(at: &str) -> String {
    let Ok(then) = OffsetDateTime::parse(at, &Rfc3339) else {
        return at.to_string();
    };
    let then = then.to_offset(time::UtcOffset::UTC);
    let today = OffsetDateTime::now_utc().date();
    if then.date() == today {
        format!("{:02}:{:02}Z", then.hour(), then.minute())
    } else {
        format!(
            "{:02}-{:02} {:02}:{:02}Z",
            u8::from(then.month()),
            then.day(),
            then.hour(),
            then.minute()
        )
    }
}
