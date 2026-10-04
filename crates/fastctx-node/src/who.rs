//! `fastctx w` on this machine: each resident, here or not, its latest
//! action and what it has running; and what runs on with nobody here.

use std::io;

use fastctx_hands::run::record;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::About;
use fastctx_types::town::{ResidentNow, RunningJob, WhoReply};
use fastctx_types::{MachineName, ResidentId, TownName};

use crate::jobs::first_line;
use crate::residents::last_action;

pub fn who(
    world: &WorldDir,
    machine: &MachineName,
    everyone: Vec<(ResidentId, Option<TownName>, bool)>,
    all: bool,
) -> io::Result<WhoReply> {
    let mut residents = Vec::new();
    let mut unattended = Vec::new();
    for (id, town, here) in everyone {
        let directory = world.resident(town.as_ref(), &id);
        let running = running_jobs(world, &id, town.as_ref())?;
        if !here {
            unattended.extend(running.iter().cloned());
            if !all {
                continue;
            }
        }
        let about: About = serde_json::from_slice(&std::fs::read(directory.about())?).map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", directory.about().display()))
        })?;
        let last_action = last_action(world, &id, town.as_ref())?;
        residents.push(ResidentNow { about, town, here, last_action, running });
    }
    Ok(WhoReply { machine: machine.clone(), residents, unattended })
}

/// The jobs of a resident that have not ended.
fn running_jobs(world: &WorldDir, resident: &ResidentId, town: Option<&TownName>) -> io::Result<Vec<RunningJob>> {
    let jobs = world.resident(town, resident).jobs();
    let entries = match std::fs::read_dir(&jobs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut running = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let job = record::read(&path)?;
        if job.end.is_none() {
            running.push(RunningJob {
                job: job.id,
                machine: job.machine,
                resident: job.resident,
                started_at: job.started_at,
                pid: job.pid,
                command: first_line(&job.script),
                output: job.output,
            });
        }
    }
    running.sort_by_key(|job| job.started_at);
    Ok(running)
}
