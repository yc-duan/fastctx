//! Current activity and the facts recorded since the caller's previous turn.

mod detail;
mod head;
mod history;
mod render;
mod sentinels;

use clap::Args;
use fastctx_client::town::{MachineState, Scope, address, busy};
use fastctx_client::{Client, ConnectError, TownView, World};
use fastctx_hands::told::{duration, turn_end};
use fastctx_types::residents::{Activity, Host, MessageKind, Origin};
use fastctx_types::run::{EndedHow, ExitStatus, JobRecord};
use fastctx_types::town::{Looked, LookedUp, ResidentDetail, ResidentNow};
use fastctx_types::{Address, Call, JobId, MachineName, Reply, ResidentId, TownName};
use jiff::Timestamp;

/// See current work and what happened while you were away.
#[derive(Args)]
pub struct WArgs {
    /// Also list sessions that are idle or asleep but can still be messaged, with their last action.
    #[arg(short = 'a', long = "all")]
    all: bool,
    /// Read history after this time: 2h, 14:00, or an RFC 3339 timestamp.
    #[arg(long)]
    since: Option<String>,
    /// Show every matching history entry instead of the most recent forty.
    #[arg(long)]
    full: bool,
    /// A project, or session and job ids. Add @MACHINE to an id when several machines have it.
    names: Vec<String>,
}

pub fn run(mut args: WArgs) -> anyhow::Result<()> {
    let now = Timestamp::now();
    let since = args.since.as_deref().map(|text| history::parse(text, now)).transpose()?;
    let world = World::here()?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let mut client = match runtime.block_on(Client::connect(world.dir(), fastctx_client::command_peer()?)) {
        Ok(client) => Some(client),
        Err(ConnectError::NoNode { .. }) => None,
        Err(error) => return Err(error.into()),
    };
    let is_id = |name: &str| {
        let id = name.split_once('@').map_or(name, |(id, _)| id);
        ResidentId::try_from(id).is_ok() || JobId::try_from(id).is_ok()
    };
    let sentinel_names: Vec<_> = args.names.iter().filter(|name| sentinels::named(name)).cloned().collect();
    let overview = args.names.is_empty() || (args.names.len() == 1 && !is_id(&args.names[0]));
    if !sentinel_names.is_empty() && let Some(client) = &client {
        anyhow::ensure!(client.sentinels, "upgrade {} to inspect sentinels", client.machine);
    }
    let (sentinel_rows, sentinel_errors) = if (overview || !sentinel_names.is_empty()) && let Some(client) = &mut client {
        sentinels::list(&runtime, client)?
    } else { (Vec::new(), Vec::new()) };
    if !sentinel_names.is_empty() {
        anyhow::ensure!(client.is_some(), "the node is not running; sentinel activity cannot be verified");
        anyhow::ensure!(args.since.is_none() && !args.full, "--since and --full do not apply to sentinel ids");
        crate::output::print(&sentinels::detail(&sentinel_names, &sentinel_rows, &sentinel_errors)?)?;
        args.names.retain(|name| !sentinels::named(name));
        if args.names.is_empty() { return Ok(()); }
    }
    let you = std::env::var("FASTCTX_RESIDENT").ok().map(|value| value.parse::<Address>()).transpose()?;
    let text = if args.names.is_empty() || (args.names.len() == 1 && !is_id(&args.names[0])) {
        let town = args
            .names
            .first()
            .map(|town| TownName::try_from(town.as_str()))
            .transpose()?
            .or_else(crate::ask::town_from_run);
        // Keep the owners of unattended jobs available to the renderer, while
        // the default presentation still contains only current work.
        let who = if let Some(client) = &mut client {
            match runtime.block_on(client.call(Call::Who { all: true, town, since }))? {
                Reply::Who(reply) => reply,
                other => anyhow::bail!("the node answered something other than activity: {other:?}"),
            }
        } else {
            fastctx_node::activity::who(world.dir(), town, true, since, you.clone())?
        };
        anyhow::ensure!(since.is_none() || who.history.is_some(), "the node did not return the requested history");
        let you = who.you.as_ref().or(you.as_ref());
        let view = TownView::new(who.clone(), you);
        let mut text = render::overview(&view, you, args.all, now, client.is_none(), &sentinel_rows);
        text.push_str(&sentinels::unavailable(&sentinel_errors));
        if let Some(history) = &who.history {
            text.push_str(&history::render(history, args.since.is_none(), args.full, now, &view));
        }
        text
    } else {
        anyhow::ensure!(
            args.since.is_none() && !args.full,
            "--since and --full apply to a town or the World, not individual ids"
        );
        let mut machines = Vec::new();
        let mut ids = Vec::new();
        for name in &args.names {
            anyhow::ensure!(is_id(name), "{name} is not a resident or job id; look at a town on its own");
            let (id, machine) = match name.split_once('@') {
                Some((id, machine)) => (id, Some(fastctx_types::access::Written::parse(machine)?)),
                None => (name.as_str(), None),
            };
            ids.push(id.to_string());
            machines.push(machine);
        }
        let mut looked = if let Some(client) = &mut client {
            match runtime.block_on(client.call(Call::Look { ids }))? {
                Reply::Looked { looked } => looked,
                other => anyhow::bail!("the node answered something other than details: {other:?}"),
            }
        } else {
            fastctx_node::activity::look(world.dir(), ids)?
        };
        for (one, machine) in looked.iter_mut().zip(machines) {
            if let Some(machine) = machine {
                let matches = |written: &fastctx_types::access::Written| {
                    written.name == machine.name
                        && machine.hub.as_ref().is_none_or(|hub| written.hub.as_ref() == Some(hub))
                };
                one.found.retain(|found| matches(&found.written()));
                if JobId::try_from(one.id.as_str()).is_ok() {
                    one.unasked.retain(|unasked| {
                        unasked
                            .split_whitespace()
                            .next()
                            .and_then(|first| fastctx_types::access::Written::parse(first).ok())
                            .is_some_and(|written| matches(&written))
                    });
                }
                one.jobs.retain(|job| {
                    job.owner.resident().is_some_and(|owner| owner.machine == machine.name)
                        && machine.hub.as_ref().is_none_or(|hub| job.written().hub.as_ref() == Some(hub))
                });
            }
        }
        looked.iter().map(|one| detail::detail(one, now)).collect()
    };
    if client.is_none() {
        crate::output::print(&format!(
            "{text}node not running: local records only; current activity is unverified and other machines were not asked.
"
        ))?;
    } else {
        crate::output::print(&text)?;
    }
    Ok(())
}

fn host(host: &Host) -> String {
    match host.agent.as_str() {
        "codex" => "Codex".into(),
        "claude" => "Claude Code".into(),
        _ => host.client.clone(),
    }
}

fn first_line(text: &str) -> String {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default().trim_end();
    if lines.next().is_some() { format!("{first} …") } else { first.to_string() }
}

fn ago(then: Timestamp, now: Timestamp) -> String {
    duration(now.as_second().saturating_sub(then.as_second()).max(0) as u64)
}

/// When something happened, as a model reads it without knowing the
/// machine's clock or zone: how long ago.
fn when(at: Timestamp, now: Timestamp) -> String {
    format!("{} ago", ago(at, now))
}
