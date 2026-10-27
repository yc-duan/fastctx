//! Read-only activity records for `w` when the local node is absent.

use std::io;
use std::path::Path;

use fastctx_types::layout::{ConversationDir, WorldDir};
use fastctx_types::project::TownSummary;
use fastctx_types::town::{MachineWho, WhoScope, WorldWho};
use fastctx_types::{Address, MachineName, TownName};
use jiff::Timestamp;

/// The latest recorded terminal boundary, using the host's timestamp when kept.
pub fn last_turn(directory: &ConversationDir) -> io::Result<Option<Timestamp>> {
    Ok(crate::delivery::terminals(directory)?
        .into_iter()
        .map(|finished| finished.ended_at.unwrap_or(finished.at))
        .max())
}

pub(crate) fn entries(path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

pub(crate) fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}

fn recorded(world: &WorldDir) -> io::Result<(Vec<crate::residents::OnRecord>, Vec<crate::residents::Former>)> {
    let mut residents = Vec::new();
    let mut former = Vec::new();
    for kept in world.kept()? {
        let fastctx_types::layout::Kind::Home(id) = &kept.kind else { continue };
        let directory = &kept.directory;
        if !directory.about().exists() {
            continue;
        }
        let about: fastctx_types::residents::About = read(&directory.about())?;
        let places = crate::residents::read_places(directory, &about)?;
        for (town, moved) in places.departures() {
            former.push(crate::residents::Former { id: id.clone(), town, to: moved.to, at: moved.at });
        }
        let now = crate::residents::read_now(directory)?;
        residents.push(crate::residents::OnRecord {
            removed: crate::residents::read_removed(directory)?,
            id: about.id.clone(),
            town: places.now().cloned(),
            here: false,
            about,
            now,
        });
    }
    Ok((residents, former))
}

/// No node is started, no resident recovered, and no records changed.
pub fn who(
    world: &WorldDir,
    town: Option<TownName>,
    all: bool,
    since: Option<Timestamp>,
    you: Option<Address>,
) -> io::Result<WorldWho> {
    #[derive(serde::Deserialize)]
    struct Machine {
        name: MachineName,
    }
    let machine: Machine = read(&world.machine_file())?;
    let (residents, former) = recorded(world)?;
    let caller =
        you.as_ref().and_then(|you| residents.iter().find(|r| r.id == you.resident && r.about.machine == you.machine));
    let town = town.or_else(|| caller.and_then(|r| r.town.clone()));
    let since = match (since, caller) {
        (Some(at), _) => Some(at),
        (None, Some(caller)) => last_turn(&world.conversation(&caller.id))?,
        _ => None,
    };
    let scope = match &town {
        Some(town) => {
            WhoScope::Town { town: town.clone(), record: crate::town::read::latest(world, town)?.map(Box::new) }
        }
        None => {
            let mut towns = Vec::new();
            for town in world.towns()? {
                if let Some(record) = crate::town::read::latest(world, &town)? {
                    towns.push(TownSummary {
                        journals: crate::memory::store::catalog(world, &town)?.journals.len(),
                        town,
                        record,
                    });
                }
            }
            WhoScope::World { towns }
        }
    };
    let mut local = crate::who::who(world, &machine.name, residents, former, all, town.as_ref())?;
    if let Some(since) = since {
        local.history = Some(crate::since::local_records(world, &machine.name, since, town.as_ref())?);
    }
    let mut who = WorldWho {
        others: Vec::new(),
        machine: machine.name,
        scope,
        machines: vec![MachineWho::Answered { observation: None, reply: local }],
        you,
        history: None,
    };
    if let Some(since) = since {
        who.collect_history(since, town);
    }
    Ok(who)
}

pub fn look(world: &WorldDir, ids: Vec<String>) -> io::Result<Vec<fastctx_types::town::LookedUp>> {
    let (residents, _) = recorded(world)?;
    ids.into_iter()
        .map(|id| {
            let record = residents.iter().find(|r| r.id.as_str() == id);
            let found = crate::look::read_here(world, &id, record, Vec::new())?.into_iter().collect();
            let jobs = crate::look::jobs_for(world, &id)?;
            let mut one = fastctx_types::town::LookedUp { id, found, unasked: Vec::new(), jobs };
            one.attach_jobs();
            Ok(one)
        })
        .collect()
}
