//! `fastctx w` on this machine: each resident living here, here now or not,
//! its latest action; and every job still running here, whoever started it.

use std::io;
use std::path::Path;

use fastctx_hands::run::record;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{Activity, Moved};
use fastctx_types::town::{ResidentNow, RunningJob, WhoReply};
use fastctx_types::{MachineName, TownName};

use crate::jobs::first_line;
use crate::residents::{Former, OnRecord, last_action};

pub fn who(
    world: &WorldDir,
    machine: &MachineName,
    everyone: impl IntoIterator<Item = OnRecord>,
    former: Vec<Former>,
    all: bool,
    town: Option<&TownName>,
) -> io::Result<WhoReply> {
    let mut residents = Vec::new();
    for resident in everyone {
        if town.is_some_and(|town| resident.town.as_ref() != Some(town)) {
            continue;
        }
        let last_action = last_action(world, &resident.id)?;
        // A conversation the World started is listed from its first words on;
        // a session the user opened elsewhere once it has done something here.
        // A first World run is already work before it has an action result.
        let participated = last_action.is_some()
            || resident.about.world_managed()
            || matches!(resident.about.origin, fastctx_types::residents::Origin::Colleague { .. })
            || resident.now.as_ref().is_some_and(|now| now.last_tool_at.is_some() || now.desk || !now.owed.is_empty())
            || read_dir(&world.conversation(&resident.id).jobs())?
                .iter()
                .any(|path| path.extension().is_some_and(|ext| ext == "json"));
        if !participated
            || (!all && (!resident.here || resident.now.as_ref().is_some_and(|now| now.activity != Activity::Busy)))
        {
            continue;
        }
        residents.push(ResidentNow {
            source: None,
            about: resident.about,
            town: resident.town,
            here: resident.here,
            now: resident.now,
            last_action,
            tools: Vec::new(),
            moved: None,
            removed: resident.removed,
        });
    }
    // Those who moved away are listed in the town they left, with all: once,
    // as they last left it, with what they last did there.
    if all && let Some(town) = town {
        let mut left: Vec<Former> = Vec::new();
        for former in former.into_iter().filter(|former| former.town.as_ref() == Some(town)) {
            match left.iter_mut().find(|prior| prior.id == former.id) {
                Some(prior) if prior.at < former.at => *prior = former,
                Some(_) => {}
                None => left.push(former),
            }
        }
        for former in left {
            let directory = world.conversation(&former.id);
            let about = crate::residents::read_about(&directory)?;
            let places = crate::residents::read_places(&directory, &about)?;
            if places.now() == Some(town) {
                continue;
            }
            let mut last_action = None;
            for (from, until) in places.spans(Some(town)).into_iter().rev() {
                last_action = crate::residents::last_action_before(&directory.actions(), Some(from), until)?;
                if last_action.is_some() {
                    break;
                }
            }
            let now = crate::residents::read_now(&directory)?;
            if last_action.is_none()
                && !matches!(about.origin, fastctx_types::residents::Origin::Colleague { .. })
                && now.as_ref().is_none_or(|now| now.last_tool_at.is_none())
            {
                continue;
            }
            residents.push(ResidentNow {
                source: None,
                about,
                town: former.town,
                here: false,
                now: None,
                last_action,
                tools: Vec::new(),
                removed: crate::residents::removal(world, &former.id)?,
                moved: Some(Moved { to: former.to, at: former.at }),
            });
        }
    }
    let mut jobs = Vec::new();
    for parent in world.job_parents()? {
        for directory in read_dir(&parent)? {
            running_jobs(&directory.join("jobs"), &mut jobs)?;
        }
    }
    if let Some(town) = town {
        jobs.retain(|job| job.town.as_ref() == Some(town));
    }
    jobs.sort_by_key(|job| job.started_at);
    Ok(WhoReply { machine: machine.clone(), residents, jobs, history: None, busy: None })
}

/// Marks what `reply` shows that works for no account `acting` works for,
/// as this computer's views of its hubs say.
pub(crate) fn mark_others(node: &crate::Node, acting: &Acting, reply: &mut fastctx_types::town::WorldWho) {
    reply.mark_others(
        |account| node.serves(acting, &account.hub, &account.account),
        |machine, source| {
            node.links()
                .iter()
                .filter(|link| source.is_none_or(|source| link.membership().id == source.hub))
                .flat_map(|link| {
                    let hub = link.membership().id.clone();
                    let logins = link.view().member(machine).map(|member| member.logins.clone());
                    logins.into_iter().flatten().map(move |account| HubAccount { hub: hub.clone(), account })
                })
                .collect()
        },
    );
}

/// The jobs in one resident's `jobs` directory that have not ended.
pub(crate) fn running_jobs(jobs: &Path, running: &mut Vec<RunningJob>) -> io::Result<()> {
    for path in read_dir(jobs)? {
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let job = match record::read(&path) {
            Ok(job) => job,
            // Let go of since it was listed, as an ended job's record is: it
            // runs no more, and the others are still read.
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if job.end.is_none() && !job.is_sentinel() {
            let account = job.on.and_then(|on| on.account).or(job.account);
            running.push(RunningJob {
                source: None,
                job: job.id,
                town: job.town,
                machine: job.machine,
                owner: job.owner,
                started_at: job.started_at,
                pid: job.pid,
                command: first_line(&job.script),
                output: job.output,
                stderr: job.stderr,
                account,
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
