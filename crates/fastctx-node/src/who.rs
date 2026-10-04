//! `fastctx w` on this machine: each resident living here, here now or not,
//! its latest action; and every job still running here, whoever started it.

use std::io;
use std::path::Path;

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
    for (id, town, here) in everyone {
        if !here && !all {
            continue;
        }
        let directory = world.resident(town.as_ref(), &id);
        let about: About = serde_json::from_slice(&std::fs::read(directory.about())?).map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", directory.about().display()))
        })?;
        let last_action = last_action(world, &id, town.as_ref())?;
        residents.push(ResidentNow { about, town, here, last_action });
    }
    let mut jobs = Vec::new();
    for parent in world.resident_parents()? {
        for directory in read_dir(&parent)? {
            running_jobs(&directory.join("jobs"), &mut jobs)?;
        }
    }
    jobs.sort_by_key(|job| job.started_at);
    Ok(WhoReply { machine: machine.clone(), residents, jobs })
}

/// The jobs in one resident's `jobs` directory that have not ended.
fn running_jobs(jobs: &Path, running: &mut Vec<RunningJob>) -> io::Result<()> {
    for path in read_dir(jobs)? {
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let job = record::read(&path)?;
        if job.end.is_none() {
            running.push(RunningJob {
                job: job.id,
                machine: job.machine,
                owner: job.owner,
                started_at: job.started_at,
                pid: job.pid,
                command: first_line(&job.script),
                output: job.output,
            });
        }
    }
    Ok(())
}

fn read_dir(path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
