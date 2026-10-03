//! `fastctx w`: who is in this town and what they are doing (`specs/world-town.md`).

use super::who::{self, Query};
use super::w::{self, View};
use crate::control::paths::ControlPaths;
use crate::world::WorldPaths;
use std::path::PathBuf;
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

pub(crate) async fn run(args: WArgs) -> Result<ExitCode, String> {
    let paths = WorldPaths::from_control(&ControlPaths::discover()?);
    let config = crate::world::load_config(&paths)?.ok_or_else(|| {
        "This machine is not in a World; fastctx w looks at the residents of a World's towns.".to_string()
    })?;
    let caller = std::env::var(RESIDENT_VARIABLE).ok().filter(|id| !id.is_empty());
    let (ids, towns): (Vec<String>, Vec<String>) = args.targets.into_iter().partition(|target| is_id(target));
    let query = match (ids.is_empty(), towns.as_slice()) {
        (false, []) => Query { ids, ..Query::default() },
        (true, []) => Query { town: own_town(&paths.dir, &config.name, caller.as_deref()), all: args.all, ..Query::default() },
        (true, [town]) => Query { town: Some(town.clone()), all: args.all, ..Query::default() },
        _ => return Err("fastctx w takes one town, or one or more resident and job ids.".to_string()),
    };
    // The node asks every other machine the query covers; without it, only this machine's
    // records can be read, and the view says so.
    let (answers, silent, alone) = match gather(&query).await? {
        Some((answers, silent)) => (answers, silent, false),
        None => (vec![who::answer(&paths.dir, &config.name, &query)], Vec::new(), true),
    };
    let view = View { caller: caller.as_deref(), query: &query, answers: &answers, silent: &silent };
    print!("{}", w::render(&view, time::OffsetDateTime::now_utc()));
    if alone {
        println!("(only {}: its node is not running, so no other machine was asked)", config.name);
    }
    Ok(ExitCode::SUCCESS)
}

/// The answers of every machine the query covers, as the node gathers them; `None` when the
/// node is not running.
pub(crate) async fn gather(query: &Query) -> Result<Option<(Vec<who::MachineAnswer>, Vec<w::Silent>)>, String> {
    use crate::world::node::admin;
    let environment = crate::session::SessionEnvironment::capture()?;
    let endpoint = crate::runtime::node_admin_endpoint(&environment)?;
    let request = admin::AdminRequest::Town { request: super::admin::TownRequest::Who { query: query.clone() } };
    let wait = super::remote::calls::CALL_TIMEOUT + admin::ADMIN_TIMEOUT;
    let Some(response) = admin::call(&endpoint, &request, wait).await? else { return Ok(None) };
    if !response.ok {
        return Err(response.error.unwrap_or_else(|| "The node refused the request.".to_string()));
    }
    let field = |name: &str| response.data.get(name).cloned().unwrap_or_default();
    let answers = serde_json::from_value(field("answers")).map_err(|error| format!("Unreadable answers from the node: {error}"))?;
    let silent = serde_json::from_value(field("silent")).map_err(|error| format!("Unreadable answers from the node: {error}"))?;
    Ok(Some((answers, silent)))
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
    who::current(found.residents.iter().collect()).and_then(|facts| facts.about.town.clone())
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
    /// Open a project: its town, what it is, its machines and where things are. Only when the
    /// user asks for it.
    New {
        /// The town's short name: lowercase letters, digits and hyphens, such as mark50.
        town: String,
        /// The project's name, in any words.
        #[arg(long)]
        name: String,
        /// One or two sentences on what the project is for.
        #[arg(long)]
        what: String,
        /// The machines the town spans, separated by commas.
        #[arg(long, value_delimiter = ',', required = true)]
        machines: Vec<String>,
        /// One line of the address book: what it is, the machine, and its native absolute path.
        /// Repeat for each line.
        #[arg(long, num_args = 3, value_names = ["WHAT", "MACHINE", "PATH"])]
        place: Vec<String>,
        /// Where the canon, the specs the user confirms, lives: the machine and the folder.
        #[arg(long, num_args = 2, value_names = ["MACHINE", "PATH"])]
        canon: Vec<String>,
    },
    /// Write the next version of a project's charter. What is left out stays as it was; any
    /// --place replaces the whole address book.
    Edit {
        /// The town; omitted, the one you are in.
        town: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        what: Option<String>,
        #[arg(long, value_delimiter = ',')]
        machines: Option<Vec<String>>,
        #[arg(long, num_args = 3, value_names = ["WHAT", "MACHINE", "PATH"])]
        place: Vec<String>,
        #[arg(long, num_args = 2, value_names = ["MACHINE", "PATH"])]
        canon: Vec<String>,
        /// The project no longer names where its canon lives.
        #[arg(long, conflicts_with = "canon")]
        no_canon: bool,
    },
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
    /// Add an entry to a journal from a draft file. A new journal also needs --description.
    Add {
        /// The journal: lowercase words joined by hyphens, naming a thing.
        name: String,
        /// The draft holding the entry's text; - reads it from standard input.
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        /// What a new journal covers: its scope, not what it found.
        #[arg(long)]
        description: Option<String>,
        /// An earlier entry this one answers. Repeat for each.
        #[arg(long = "cite", value_name = "ENTRY")]
        cites: Vec<String>,
        /// A file to attach as evidence. Repeat for each.
        #[arg(long = "attach", value_name = "FILE")]
        attachments: Vec<PathBuf>,
        /// A file on another machine of the World to attach: the machine, then the file's
        /// path there. Repeat for each.
        #[arg(long = "attach-from", num_args = 2, value_names = ["MACHINE", "FILE"])]
        attach_from: Vec<String>,
        #[arg(long)]
        town: Option<String>,
    },
    /// Change what a journal says it covers. It becomes a new version; the earlier ones stay.
    Describe {
        name: String,
        /// What the journal covers: its scope, not what it found.
        text: String,
        #[arg(long)]
        town: Option<String>,
    },
}

/// How residents write journals, shown with `fastctx journal --help` (`specs/world-journals.md`,
/// "写给居民的提示").
pub(crate) const JOURNAL_PROMPT: &str = "\
This project keeps journals. They are history, not verdicts. Write only
sentences that will still be true a year from now, whatever is found later.

Add an entry when something happened that the next resident here needs and
cannot recover from the specs, the code, or git:
  - you tried something and it did not go as you meant, or you turned to
    another path;
  - something you were trying to find out came out one way or another;
  - you are stopping, handing off, or stuck: where the work is, what fails,
    and where it can be picked up.
Routine actions are not history. A few entries across one or several
context compactions is typical.

Record what you did, what you meant to find out, and what you observed,
with the conditions (version, machine, input, settings) and the evidence
attached. \"On WebRTC 1.2 over a 3G profile, packet loss measured 35% (log
attached)\" stays true. \"WebRTC is not viable here\" may not; do not write
it. Do not write verdicts such as \"the cause is\", \"does not work\", \"done\",
or \"the right approach\". Questions you could not answer are welcome.
Do not write relative time such as \"now\", \"currently\", \"recently\", or
\"yesterday\"; each entry is stamped with its time, so write dates and
versions. Quote the user's words verbatim. Cite code as path@commit or
attach the lines; the next reader may be on another machine.

Before starting a journal, look for one on the same subject and add to it,
even if what you saw contradicts it. Name a new journal after a thing,
never after a conclusion; keep its description to what it covers, not what
it found. Earlier entries record what someone saw under their conditions.
Do not defer to them because they are older or because another model wrote
them: test them, and add what you see. When your entry answers an earlier
one, cite that entry's id. Never change an entry, yours included; add a new
one. Journal files are read-only copies; write only through fastctx journal.";

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
        ProjectCommand::New { town, name, what, machines, place, canon } => {
            let change = super::ledger::CharterChange {
                name: Some(name),
                what: Some(what),
                machines: Some(machines),
                places: Some(places(&place)),
                canon: location(&canon),
                no_canon: false,
            };
            write_charter(town, change, true, caller).await?;
        }
        ProjectCommand::Edit { town, name, what, machines, place, canon, no_canon } => {
            let town = chosen_town(&paths.dir, &machine, caller.as_deref(), town)?;
            let change = super::ledger::CharterChange {
                name,
                what,
                machines,
                places: (!place.is_empty()).then(|| places(&place)),
                canon: location(&canon),
                no_canon,
            };
            write_charter(town, change, false, caller).await?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `--place WHAT MACHINE PATH`, repeated, as address book lines.
fn places(values: &[String]) -> Vec<super::project::Place> {
    values
        .chunks(3)
        .filter_map(|line| match line {
            [what, machine, path] => Some(super::project::Place {
                what: what.clone(),
                location: super::project::Location { machine: machine.clone(), path: path.clone() },
            }),
            _ => None,
        })
        .collect()
}

/// `--canon MACHINE PATH`.
fn location(values: &[String]) -> Option<super::project::Location> {
    match values {
        [machine, path] => Some(super::project::Location { machine: machine.clone(), path: path.clone() }),
        _ => None,
    }
}

async fn write_charter(town: String, change: super::ledger::CharterChange, opening: bool, resident: Option<String>) -> Result<(), String> {
    let request = crate::world::node::admin::AdminRequest::Town {
        request: super::admin::TownRequest::Charter { town: town.clone(), change, opening, resident },
    };
    let data = ask_node(&request).await?;
    let version = data.get("version").and_then(serde_json::Value::as_u64).unwrap_or_default();
    println!("=== project {town}: charter version {version} written ===");
    print!("{}", data.get("charter").and_then(serde_json::Value::as_str).unwrap_or_default());
    Ok(())
}

pub(crate) async fn run_journal(command: JournalCommand) -> Result<ExitCode, String> {
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
        JournalCommand::Add { name, file, description, cites, attachments, attach_from, town } => {
            let town = chosen_town(&paths.dir, &machine, caller.as_deref(), town)?;
            let text = if file.as_os_str() == "-" {
                std::io::read_to_string(std::io::stdin()).map_err(|error| format!("Cannot read the draft from standard input: {error}"))?
            } else {
                std::fs::read_to_string(&file).map_err(|error| format!("Cannot read the draft {}: {error}", crate::paths::display_path(&file)))?
            };
            // The node reads the attachments, from where this command was run.
            let attachments = attachments
                .iter()
                .map(|path| std::path::absolute(path).map_err(|error| format!("Cannot resolve {}: {error}", path.display())))
                .collect::<Result<Vec<_>, _>>()?;
            // A file named on this machine is found from here, like one attached with --attach.
            let remote_attachments = attach_from
                .chunks(2)
                .map(|pair| {
                    let path = if pair[0] == machine {
                        let absolute = std::path::absolute(&pair[1]).map_err(|error| format!("Cannot resolve {}: {error}", pair[1]))?;
                        crate::paths::display_path(&absolute)
                    } else {
                        pair[1].clone()
                    };
                    Ok(super::ledger::RemoteFile { machine: pair[0].clone(), path })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let draft = super::ledger::EntryDraft { town: town.clone(), journal: name.clone(), text, description, cites, attachments, remote_attachments };
            let request = crate::world::node::admin::AdminRequest::Town { request: super::admin::TownRequest::JournalAdd { draft, resident: caller } };
            let data = ask_node(&request).await?;
            let entry = data.get("entry").and_then(serde_json::Value::as_str).unwrap_or_default();
            println!("=== journal {name} · {town}: entry {entry} added ===");
        }
        JournalCommand::Describe { name, text, town } => {
            let town = chosen_town(&paths.dir, &machine, caller.as_deref(), town)?;
            let request = crate::world::node::admin::AdminRequest::Town {
                request: super::admin::TownRequest::Describe { town: town.clone(), journal: name.clone(), text, resident: caller },
            };
            let data = ask_node(&request).await?;
            let version = data.get("version").and_then(serde_json::Value::as_u64).unwrap_or_default();
            println!("=== journal {name} · {town}: description version {version} written ===");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The run engine's end hook: tells the node that the job whose record is given has ended.
pub(crate) async fn run_job_ended(record: std::path::PathBuf) -> Result<ExitCode, String> {
    let request = crate::world::node::admin::AdminRequest::Town { request: super::admin::TownRequest::JobEnded { record } };
    ask_node(&request).await?;
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
