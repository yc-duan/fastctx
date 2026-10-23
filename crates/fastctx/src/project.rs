//! `fastctx project`: the towns, their records and maps, and entering one
//! (`world-projects.md`).

use clap::{ArgAction, Args, Subcommand};
use fastctx_hands::town::{brief, map, record_stamp, stamped};
use fastctx_types::project::{Listing, Location, Record, RecordFields, Stamp, TownSummary};
use fastctx_types::{Call, MachineName, ProjectCall, Reply, TownName};

use crate::ask;

/// The user's projects: list, show and enter them; open one or write
/// the next version of its record.
#[derive(Subcommand)]
pub enum ProjectCommand {
    /// The projects this machine is part of.
    List,
    #[command(about = fastctx_types::memory::words::SHOW_HELP)]
    Show {
        /// The project; without it, the one you work in.
        #[arg(value_name = "NAME")]
        town: Option<String>,
    },
    /// Work in a project from now on, and get its record. Only when the user
    /// says which project to work on; do not change projects on your own.
    Enter {
        /// The project's short name, as `fastctx project list` shows it.
        #[arg(value_name = "NAME")]
        town: String,
    },
    /// Open a project: the first version of its record. Only when the user
    /// asks for a new project, or agrees to one.
    New {
        /// Its short name: lowercase letters, digits and hyphens.
        #[arg(value_name = "NAME")]
        town: String,
        #[command(flatten)]
        fields: Fields,
        /// The account it is opened in, where this works for more than one.
        #[arg(long = "as")]
        account: Option<fastctx_types::AccountName>,
        /// The hub of that account, where several have one of its name.
        #[arg(long)]
        hub: Option<fastctx_types::HubName>,
    },
    /// Write the next version of a project's record; what is not given
    /// stays as it is.
    Edit {
        /// The project; without it, the one you work in.
        #[arg(value_name = "NAME")]
        town: Option<String>,
        #[command(flatten)]
        fields: Fields,
    },
}

#[derive(Args)]
pub struct Fields {
    /// The project's name, in any words.
    #[arg(long)]
    name: Option<String>,
    /// The user's instructions for all work in the project, which every
    /// agent working in it is given: what it is for, how to work in it,
    /// what to keep in mind. Written as the user words them.
    #[arg(long, value_name = "TEXT")]
    what: Option<String>,
    /// The machines the project spans, separated by commas. Opening a
    /// project without it, the project spans this machine; editing without
    /// it, the machines stay as they are.
    #[arg(long, value_delimiter = ',')]
    machines: Option<Vec<String>>,
    /// One line of the address book: what is there, the machine, and the
    /// absolute path. Given once or more, it replaces the whole book.
    #[arg(long, num_args = 3, value_names = ["WHAT", "MACHINE", "PATH"], action = ArgAction::Append)]
    place: Vec<String>,
}

pub fn run(command: ProjectCommand) -> anyhow::Result<()> {
    let town = |given: Option<String>| -> anyhow::Result<Option<TownName>> {
        Ok(match given {
            Some(town) => Some(TownName::try_from(town)?),
            None => ask::town_from_run(),
        })
    };
    let call = match command {
        ProjectCommand::List => ProjectCall::List,
        ProjectCommand::Show { town: given } => ProjectCall::Map { town: town(given)? },
        ProjectCommand::Enter { town } => ProjectCall::Enter { town: TownName::try_from(town)? },
        ProjectCommand::New { town, fields, account, hub } => {
            ProjectCall::New { town: TownName::try_from(town)?, fields: fields.into_fields()?, account, hub }
        }
        ProjectCommand::Edit { town: given, fields } => {
            ProjectCall::Edit { town: town(given)?, fields: fields.into_fields()? }
        }
    };
    let opening = matches!(call, ProjectCall::New { .. });
    let editing = matches!(call, ProjectCall::Edit { .. });
    let began = matches!(call, ProjectCall::Enter { .. }).then(crate::steps::began).flatten();
    let (reply, here) = ask::call(Call::Project(call))?;
    let text = match reply {
        Reply::Towns { towns } => list(&here, &towns),
        Reply::Record { town, record, opened } => {
            let title = if opening {
                format!("opened the project {town}")
            } else if editing {
                format!("wrote version {} of {town}'s record", record.version)
            } else {
                format!("project {town}, version {} of its record", record.version)
            };
            show(&title, &record, &opened)
        }
        Reply::Map { map: view } => map(&view),
        Reply::Entered { town, map: view } => {
            if let Some(began) = began {
                began.tell(fastctx_types::run_facts::StepDid::Project { town: town.clone() });
            }
            format!("=== you work in the project {town} from now on ===\n{}", brief(&view))
        }
        other => anyhow::bail!("the node answered something else: {other:?}"),
    };
    crate::output::print(&text)?;
    Ok(())
}

impl Fields {
    fn into_fields(self) -> anyhow::Result<RecordFields> {
        let machines = self
            .machines
            .map(|machines| machines.iter().map(|machine| MachineName::try_from(machine.trim())).collect())
            .transpose()?;
        let places = if self.place.is_empty() {
            None
        } else {
            let places: anyhow::Result<Vec<Listing>> = self
                .place
                .chunks(3)
                .map(|place| {
                    Ok(Listing {
                        what: place[0].clone(),
                        location: Location {
                            machine: MachineName::try_from(place[1].as_str())?,
                            path: place[2].clone(),
                        },
                    })
                })
                .collect();
            Some(places?)
        };
        Ok(RecordFields { name: self.name, what: self.what, machines, places, canon: None })
    }
}

fn list(here: &MachineName, towns: &[TownSummary]) -> String {
    if towns.is_empty() {
        return format!("=== projects on {here}: none ===\n");
    }
    let mut text = format!("=== projects on {here} ({}) ===\n", towns.len());
    for town in towns {
        let record = &town.record;
        let machines: Vec<&str> = record.machines.iter().map(|machine| machine.as_str()).collect();
        text.push_str(&format!("{}  {} · {}\n", town.town, record.name, machines.join(", ")));
        // The first line of its instructions, which `show` gives whole.
        if let Some(first) = record.what.lines().map(str::trim).find(|line| !line.is_empty()) {
            text.push_str(&format!("    {first}\n"));
        }
    }
    text
}

fn show(title: &str, record: &Record, opened: &Stamp) -> String {
    let mut text = format!("=== {title} ===\n");
    text.push_str(&format!("Name: {}\n", record.name));
    let instructions = record.what.trim();
    if !instructions.is_empty() {
        text.push_str(&format!("The user's instructions for all work in this project; follow them:\n{instructions}\n"));
    }
    let machines: Vec<&str> = record.machines.iter().map(|machine| machine.as_str()).collect();
    text.push_str(&format!("Machines: {}\n", machines.join(", ")));
    if record.places.is_empty() {
        text.push_str("Where things are: nothing listed\n");
    } else {
        text.push_str("Where things are:\n");
        for place in &record.places {
            text.push_str(&format!("  {}: {} {}\n", place.what, place.location.machine, place.location.path));
        }
    }
    text.push_str(&format!("Opened by {}\n", stamped(opened)));
    if record.version > 1 {
        text.push_str(&format!("Version {} by {}\n", record.version, stamped(&record_stamp(record))));
    }
    text
}
