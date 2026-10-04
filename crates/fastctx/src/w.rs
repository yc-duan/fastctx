//! `fastctx w`: who is here and what each is doing, on every machine of the
//! World (`world-town.md`).

use std::collections::BTreeSet;

use clap::Args;
use fastctx_client::Client;
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{Action, Host, Origin};
use fastctx_types::town::{MachineWho, ResidentNow, RunningJob, WorldWho};
use fastctx_types::{Address, Call, Reply};
use jiff::Timestamp;

/// See who is here and what each is doing.
#[derive(Args)]
pub struct WArgs {
    /// Also list residents who have left: those who can be woken again.
    #[arg(short = 'a', long = "all")]
    all: bool,
}

pub fn run(args: WArgs) -> anyhow::Result<()> {
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    let you = std::env::var("FASTCTX_RESIDENT").ok().and_then(|address| address.parse::<Address>().ok());
    let reply = tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let mut client = Client::connect_or_start(&world, Peer::Command { resident: you.clone() }).await?;
        match client.call(Call::Who { all: args.all }).await? {
            Reply::Who(reply) => anyhow::Ok(reply),
            other => anyhow::bail!("the node answered something else: {other:?}"),
        }
    })?;
    print!("{}", render(&reply, you.as_ref(), Timestamp::now()));
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
    let address = |resident: &ResidentNow| Address {
        machine: resident.about.machine.clone(),
        resident: resident.about.id.clone(),
    };
    // The caller first, then whoever acted most recently.
    residents.sort_by_key(|resident| {
        let latest = resident.last_action.as_ref().map(|action| action.at).unwrap_or(resident.about.born_at);
        (Some(&address(resident)) != you, std::cmp::Reverse(latest))
    });

    let here = residents.iter().filter(|resident| resident.here).count();
    let mut facts = vec![format!("{here} here")];
    if reply.machines.len() > 1 {
        facts.push(format!("{} online", answered.iter().map(|name| name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    facts.extend(missing);
    let title = if reply.machines.len() > 1 { "w".to_string() } else { format!("w: {}", reply.machine) };
    let mut text = format!("=== {title} ({}) ===\n", facts.join("; "));

    let present: BTreeSet<Address> =
        residents.iter().filter(|resident| resident.here).map(|resident| address(resident)).collect();
    let width = residents.iter().map(|resident| resident.about.id.len()).max().unwrap_or(0);
    for resident in &residents {
        let you_mark = if Some(&address(resident)) == you { "  (you)" } else { "" };
        let since =
            if resident.here { format!("here {}", ago(resident.about.born_at, now)) } else { "left".to_string() };
        let origin = match &resident.about.origin {
            Origin::User => "opened by user".to_string(),
            Origin::Colleague { asked_by, label } => format!("for {asked_by} \"{label}\""),
        };
        text.push_str(&format!(
            "{:width$}{you_mark}  {} · {} · {origin} · {since}\n",
            resident.about.id,
            host(&resident.about.host),
            resident.about.machine,
        ));
        let own: Vec<&&RunningJob> = jobs.iter().filter(|job| job.owner == address(resident)).collect();
        let doing = match own.last() {
            Some(job) => running(job, now),
            None => match &resident.last_action {
                Some(action) => last(action, now),
                None => "no actions yet".to_string(),
            },
        };
        text.push_str(&format!("    {doing}\n"));
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

fn host(host: &Host) -> String {
    let name = if host.client.contains("codex") {
        "Codex"
    } else if host.client.contains("claude") {
        "Claude Code"
    } else {
        host.client.as_str()
    };
    if host.version.is_empty() { name.to_string() } else { format!("{name} {}", host.version) }
}

fn running(job: &RunningJob, now: Timestamp) -> String {
    let pid = job.pid.map(|pid| format!(", pid {pid}")).unwrap_or_default();
    format!("running on {} for {} ({}{pid}): {}", job.machine, ago(job.started_at, now), job.job, job.command)
}

fn last(action: &Action, now: Timestamp) -> String {
    format!("{} {} ago on {}: {} -> {}", action.hand, ago(action.at, now), action.machine, action.what, action.result)
}

/// How long since `then`, as `45s`, `3m`, `2h10m` or `4d`.
fn ago(then: Timestamp, now: Timestamp) -> String {
    let seconds = now.as_second().saturating_sub(then.as_second()).max(0);
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// A moment as the local clock reads it: the time today, the date before.
fn clock(at: Timestamp, now: Timestamp) -> String {
    let zone = jiff::tz::TimeZone::system();
    let (at, now) = (at.to_zoned(zone.clone()), now.to_zoned(zone));
    if at.date() == now.date() { at.strftime("%H:%M").to_string() } else { at.strftime("%Y-%m-%d %H:%M").to_string() }
}
