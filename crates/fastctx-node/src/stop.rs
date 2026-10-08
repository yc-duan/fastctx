//! `fastctx stop`: a job and everything it started, on whichever machine it
//! runs (`world-run.md`). A member that may run commands on a machine may
//! stop any job there: it could end those processes from its shell anyway,
//! and this way they end completely, on record, with the starter told.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::run::JobRecord;
use fastctx_types::{JobId, MachineName, Reply};

use crate::Node;

/// Stops `job` for `by`: on `machine`, or else on the one machine that has a
/// job with this id.
pub async fn stop(node: &Arc<Node>, by: String, job: JobId, machine: Option<MachineName>) -> Result<Reply, Failure> {
    let here = node.machine();
    if machine.as_ref().is_none_or(|machine| *machine == here)
        && let Some(record) = here_(node, &job, &by).await?
    {
        return Ok(Reply::JobStopped { machine: here, record: Box::new(record) });
    }
    if machine.as_ref() == Some(&here) {
        return Err(Failure::new(format!("{here} has no job {job}")));
    }
    let link = node.link().ok_or_else(|| Failure::new(format!("{here} has no job {job}")))?;
    let machines: Vec<MachineName> = match machine {
        Some(machine) => vec![machine],
        None => {
            let _ = link.refresh().await;
            let (table, presence) = link.members();
            table
                .machines()
                .filter(|member| member.name != here && crate::admission::reaches(node, &table, &member.name))
                .filter(|member| presence.iter().any(|presence| presence.name == member.name && presence.online))
                .map(|member| member.name.clone())
                .collect()
        }
    };
    let mut asks = Vec::new();
    for name in machines {
        let (link, call) = (link.clone(), RemoteCall::StopJob { job: job.clone(), by: by.clone() });
        asks.push(tokio::spawn(async move { (name.clone(), link.call(&name, call).await) }));
    }
    let mut stopped = Vec::new();
    let mut failed = Vec::new();
    for ask in asks {
        let Ok((name, answer)) = ask.await else { continue };
        match answer {
            Ok(Ok(RemoteReply::JobStopped { record: Some(record) })) => stopped.push((name, record)),
            Ok(Ok(_)) => {}
            Ok(Err(failure)) => failed.push(format!("{name}: {failure}")),
            Err(failure) => failed.push(format!("{name}: {failure}")),
        }
    }
    match stopped.len() {
        1 => {
            let (machine, record) = stopped.remove(0);
            Ok(Reply::JobStopped { machine, record })
        }
        0 if failed.is_empty() => Err(Failure::new(format!("no machine that could be asked has a job {job}"))),
        0 => Err(Failure::new(format!("no job {job} was stopped: {}", failed.join("; ")))),
        _ => Err(Failure::new(format!(
            "jobs named {job} were stopped on {}",
            stopped.iter().map(|(machine, _)| machine.to_string()).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// Stops `job` on this machine, if it has one by that id.
pub async fn here_(node: &Arc<Node>, job: &JobId, by: &str) -> Result<Option<JobRecord>, Failure> {
    let (world, wanted) = (node.world.clone(), job.clone());
    let path = tokio::task::spawn_blocking(move || find(&world, &wanted))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("looking for {job}: {error}")))?;
    match path {
        Some(path) => crate::jobs::stop(node, &path, by, "stopped with fastctx stop").await.map(Some),
        None => Ok(None),
    }
}

/// The record of the job `job` on this machine.
fn find(world: &WorldDir, job: &JobId) -> io::Result<Option<PathBuf>> {
    for parent in world.job_parents()? {
        let entries = match std::fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for resident in entries {
            let path = resident?.path().join("jobs").join(format!("{job}.json"));
            if path.exists() {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}
