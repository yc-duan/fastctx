//! `fastctx w`: who is here and what each is doing (`world-town.md`).

use clap::Args;
use fastctx_client::Client;
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{Action, Host, Origin};
use fastctx_types::town::{ResidentNow, RunningJob, WhoReply};
use fastctx_types::{Call, Reply, ResidentId};
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
    let you = std::env::var("FASTCTX_RESIDENT").ok().and_then(|id| ResidentId::try_from(id).ok());
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

fn render(reply: &WhoReply, you: Option<&ResidentId>, now: Timestamp) -> String {
    let mut residents: Vec<&ResidentNow> = reply.residents.iter().collect();
    // The caller first, then whoever acted most recently.
    residents.sort_by_key(|resident| {
        let is_you = Some(&resident.about.id) == you;
        let latest = resident.last_action.as_ref().map(|action| action.at).unwrap_or(resident.about.born_at);
        (!is_you, std::cmp::Reverse(latest))
    });
    let here = reply.residents.iter().filter(|resident| resident.here).count();
    let mut text = format!("=== w: {} ({here} here) ===\n", reply.machine);
    let width = residents.iter().map(|resident| resident.about.id.len()).max().unwrap_or(0);
    for resident in residents {
        let you_mark = if Some(&resident.about.id) == you { "  (you)" } else { "" };
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
        text.push_str(&format!("    {}\n", doing(resident, now)));
    }
    if !reply.unattended.is_empty() {
        text.push_str("unattended\n");
        for job in &reply.unattended {
            text.push_str(&format!(
                "    {}  {}  {}  {}   started by {}\n",
                job.job,
                job.machine,
                ago(job.started_at, now),
                job.command,
                job.resident
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

fn doing(resident: &ResidentNow, now: Timestamp) -> String {
    if let Some(job) = resident.running.last() {
        return running(job, now);
    }
    match &resident.last_action {
        Some(action) => last(action, now),
        None => "no actions yet".to_string(),
    }
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
