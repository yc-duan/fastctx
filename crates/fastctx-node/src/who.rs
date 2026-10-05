//! `fastctx w` on this machine: each resident living here, here now or not,
//! its latest action; and every job still running here, whoever started it.

use std::io;
use std::path::Path;

use fastctx_hands::run::record;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::Moved;
use fastctx_types::town::{ResidentNow, RunningJob, WhoReply};
use fastctx_types::{MachineName, TownName};

use crate::jobs::first_line;
use crate::residents::{Former, OnRecord, last_action};

pub fn who(
    world: &WorldDir,
    machine: &MachineName,
    everyone: Vec<OnRecord>,
    former: Vec<Former>,
    all: bool,
    town: Option<&TownName>,
) -> io::Result<WhoReply> {
    let mut residents = Vec::new();
    for resident in everyone {
        if (!resident.here && !all) || town.is_some_and(|town| resident.town.as_ref() != Some(town)) {
            continue;
        }
        let last_action = last_action(world, &resident.id, resident.town.as_ref())?;
        residents.push(ResidentNow {
            about: resident.about,
            town: resident.town,
            here: resident.here,
            now: resident.now,
            last_action,
            moved: None,
            removed: resident.removed,
        });
    }
    // Those who moved away are listed in the town they left, with all.
    if all && let Some(town) = town {
        for former in former.into_iter().filter(|former| former.town.as_ref() == Some(town)) {
            let directory = world.resident(Some(town), &former.id);
            residents.push(ResidentNow {
                about: crate::residents::read_about(&directory)?,
                town: former.town,
                here: false,
                now: None,
                last_action: last_action(world, &former.id, Some(town))?,
                removed: crate::residents::removal(world, &former.id)?,
                moved: Some(Moved { to: former.to, at: former.at }),
            });
        }
    }
    let parents = match town {
        Some(town) => vec![world.town(town).path().join("residents")],
        None => world.resident_parents()?,
    };
    let mut jobs = Vec::new();
    for parent in parents {
        for directory in read_dir(&parent)? {
            running_jobs(&directory.join("jobs"), &mut jobs)?;
        }
    }
    jobs.sort_by_key(|job| job.started_at);
    Ok(WhoReply { machine: machine.clone(), residents, jobs })
}

/// The jobs in one resident's `jobs` directory that have not ended.
pub(crate) fn running_jobs(jobs: &Path, running: &mut Vec<RunningJob>) -> io::Result<()> {
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
