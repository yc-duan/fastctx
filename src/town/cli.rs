//! `fastctx w`: who is in this town and what they are doing (`specs/world-town.md`).

use super::who::{self, Query};
use super::w::{self, View};
use crate::control::paths::ControlPaths;
use crate::world::WorldPaths;
use std::process::ExitCode;

/// The resident on whose behalf a command runs; the World's `run` sets it.
const RESIDENT_VARIABLE: &str = super::session::RESIDENT_VARIABLE;
/// The town that resident is in, when it is in one; the World's `run` sets it.
pub(crate) const TOWN_VARIABLE: &str = "FASTCTX_TOWN";

#[derive(Debug, clap::Args)]
pub(crate) struct WArgs {
    /// Also list residents that are asleep or have left: earlier colleagues you can wake.
    #[arg(short = 'a', long = "all")]
    all: bool,
    /// A town to look at instead of your own, or resident and job ids to see in detail.
    #[arg(value_name = "TOWN | ID")]
    targets: Vec<String>,
}

pub(crate) fn run(args: WArgs) -> Result<ExitCode, String> {
    let paths = WorldPaths::from_control(&ControlPaths::discover()?);
    let config = crate::world::load_config(&paths)?.ok_or_else(|| {
        "This machine is not in a World; fastctx w looks at the residents of a World's towns.".to_string()
    })?;
    let caller = std::env::var(RESIDENT_VARIABLE).ok().filter(|id| !id.is_empty());
    let (ids, towns): (Vec<String>, Vec<String>) = args.targets.into_iter().partition(|target| is_id(target));
    let query = match (ids.is_empty(), towns.as_slice()) {
        (false, []) => Query { ids, ..Query::default() },
        (true, []) => Query { town: own_town(&paths.dir, &config.name, caller.as_deref()), all: args.all, ids: Vec::new() },
        (true, [town]) => Query { town: Some(town.clone()), all: args.all, ids: Vec::new() },
        _ => return Err("fastctx w takes one town, or one or more resident and job ids.".to_string()),
    };
    let answers = vec![who::answer(&paths.dir, &config.name, &query)];
    let view = View { caller: caller.as_deref(), query: &query, answers: &answers, silent: &[] };
    print!("{}", w::render(&view, time::OffsetDateTime::now_utc()));
    Ok(ExitCode::SUCCESS)
}

/// Resident ids look like `codex-7f3a`, job ids like `j-k3x9a2`; town names are free words.
fn is_id(target: &str) -> bool {
    super::records::is_resident_id(target) || (target.starts_with("j-") && target.len() > 2)
}

/// The caller's town: as `run` passed it, or from its records when it lives on this machine.
fn own_town(world_dir: &std::path::Path, machine: &str, caller: Option<&str>) -> Option<String> {
    if let Ok(town) = std::env::var(TOWN_VARIABLE) {
        return Some(town).filter(|town| !town.is_empty());
    }
    let caller = caller?;
    let found = who::answer(world_dir, machine, &Query { ids: vec![caller.to_string()], ..Query::default() });
    found.residents.into_iter().next().and_then(|facts| facts.about.town)
}

/// Projects: each project is a town.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum ProjectCommand {
    /// List the towns this machine holds.
    List,
    /// Print a project's charter: what it is, its machines, and where things are.
    Show {
        /// The town; omitted, the one you are in.
        town: Option<String>,
    },
    /// Print a project's map: its charter, its journals and its canon.
    Map {
        /// The town; omitted, the one you are in.
        town: Option<String>,
    },
    /// Enter a town, or move to another one, and get its map. Only when the user tells you
    /// which project to work on.
    Enter { town: String },
}

/// Journals, the project's history.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum JournalCommand {
    /// List the journals of a town.
    List {
        #[arg(long)]
        town: Option<String>,
    },
    /// Read a journal: its description and its entries, newest that fit, in time order.
    Read {
        name: String,
        #[arg(long)]
        town: Option<String>,
        /// Show the entries written before this entry id.
        #[arg(long, value_name = "ENTRY")]
        before: Option<String>,
    },
}

pub(crate) async fn run_project(command: ProjectCommand) -> Result<ExitCode, String> {
    let (paths, machine) = enrolled()?;
    let caller = std::env::var(RESIDENT_VARIABLE).ok().filter(|id| !id.is_empty());
    match command {
        ProjectCommand::List => {
            let towns = super::project::towns(&paths.dir);
            if towns.is_empty() {
                println!("=== projects on {machine}: none ===");
            } else {
                println!("=== projects on {machine} ({}) ===", towns.len());
                for town in towns {
                    match super::project::charter(&paths.dir, &town) {
                        Ok(charter) => println!("{town}  {}: {}", charter.name, charter.what),
                        Err(error) => println!("{town}  ({error})"),
                    }
                }
            }
        }
        ProjectCommand::Show { town } => {
            let town = chosen_town(&paths.dir, &machine, caller.as_deref(), town)?;
            print!("{}", super::project::render_charter(&town, &super::project::charter(&paths.dir, &town)?));
        }
        ProjectCommand::Map { town } => {
            let town = chosen_town(&paths.dir, &machine, caller.as_deref(), town)?;
            print!("{}", super::project::render_map(&paths.dir, &town, &machine)?);
        }
        ProjectCommand::Enter { town } => {
            let resident = caller.ok_or_else(|| {
                "fastctx project enter moves the resident that runs it into a town; run it with the World's run tool.".to_string()
            })?;
            let request = crate::world::node::admin::AdminRequest::Town {
                request: super::admin::TownRequest::Enter { resident, town },
            };
            let data = ask_node(&request).await?;
            print!("{}", data.get("map").and_then(serde_json::Value::as_str).unwrap_or_default());
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn run_journal(command: JournalCommand) -> Result<ExitCode, String> {
    let (paths, machine) = enrolled()?;
    let caller = std::env::var(RESIDENT_VARIABLE).ok().filter(|id| !id.is_empty());
    match command {
        JournalCommand::List { town } => {
            let town = chosen_town(&paths.dir, &machine, caller.as_deref(), town)?;
            let summaries = super::journal::summaries(&paths.dir, &town)?;
            println!("=== journals of {town} ({}) ===", summaries.len());
            for summary in summaries {
                println!("{}  {} ({} entries)", summary.slug, summary.description, summary.entries);
            }
        }
        JournalCommand::Read { name, town, before } => {
            let town = chosen_town(&paths.dir, &machine, caller.as_deref(), town)?;
            print!("{}", super::journal::render(&paths.dir, &town, &name, before.as_deref())?);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn enrolled() -> Result<(WorldPaths, String), String> {
    let paths = WorldPaths::from_control(&ControlPaths::discover()?);
    let config = crate::world::load_config(&paths)?
        .ok_or_else(|| "This machine is not in a World; projects and journals live in a World's towns.".to_string())?;
    Ok((paths, config.name))
}

/// The town named on the command line, else the caller's own.
fn chosen_town(world_dir: &std::path::Path, machine: &str, caller: Option<&str>, named: Option<String>) -> Result<String, String> {
    named
        .or_else(|| own_town(world_dir, machine, caller))
        .ok_or_else(|| "You are outside every town; name one. fastctx project list shows the towns on this machine.".to_string())
}

async fn ask_node(request: &crate::world::node::admin::AdminRequest) -> Result<serde_json::Value, String> {
    let environment = crate::session::SessionEnvironment::capture()?;
    let endpoint = crate::runtime::node_admin_endpoint(&environment)?;
    let response = crate::world::node::admin::call(&endpoint, request, crate::world::node::admin::ADMIN_TIMEOUT)
        .await?
        .ok_or_else(|| "The FastCtx node service is not running on this machine; run 'fastctx node status'.".to_string())?;
    if response.ok { Ok(response.data) } else { Err(response.error.unwrap_or_else(|| "The node refused the request.".to_string())) }
}
