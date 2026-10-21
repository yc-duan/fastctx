//! `fastctx stop`: a job and everything it started, on whichever machine it
//! runs (`world-run.md`). A member that may run commands on a machine may
//! stop any job there: it could end those processes from its shell anyway,
//! and this way they end completely, on record, with the starter told.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::run::JobRecord;
use fastctx_types::{JobId, Reply};

use crate::Node;
use crate::around::Heard;

/// Stops `job` for `by`, who works for `acting`: on `machine`, or else on
/// the one machine it may work on that has a job with this id.
pub async fn stop(
    node: &Arc<Node>,
    acting: &Acting,
    works_here: bool,
    by: String,
    job: JobId,
    machine: Option<crate::reach::Written>,
) -> Result<Reply, Failure> {
    let here = node.machine();
    // Only whoever works on this machine stops what runs here
    // (`around::works_here`); someone at a managed machine may not.
    let may_here = works_here && !crate::managed::is_managed(node);
    if may_here
        && machine.as_ref().is_none_or(|machine| node.is_here(machine))
        && let Some(record) = here_(node, &job, &by).await?
    {
        return Ok(Reply::JobStopped { machine: here, record: Box::new(record) });
    }
    if machine.as_ref().is_some_and(|machine| node.is_here(machine)) {
        return Err(Failure::new(format!("{here} has no job {job}")));
    }
    let call = RemoteCall::StopJob { job: job.clone(), by: by.clone() };
    let heard = match machine {
        Some(written) => {
            let answer = match node.call(acting, &written, Purpose::Hands, call).await {
                Ok(Ok(reply)) => Heard::Answered(reply),
                Ok(Err(failure)) => Heard::NoAnswer(failure.message),
                Err(failure) => Heard::NoAnswer(failure.to_string()),
            };
            vec![(written, answer)]
        }
        None => {
            if !node.connected_anywhere() {
                return Err(Failure::new(format!("{here} has no job {job}")));
            }
            let machines = crate::around::reachable(node, acting, Purpose::Hands);
            crate::around::ask_each(machines, |_| call.clone())
                .await
                .into_iter()
                .map(|(asked, heard)| (asked.source.written, heard))
                .collect()
        }
    };
    let mut stopped = Vec::new();
    let mut failed = Vec::new();
    for (name, heard) in heard {
        match heard {
            Heard::Answered(RemoteReply::JobStopped { record: Some(record) }) => stopped.push((name, record)),
            Heard::Answered(_) | Heard::Offline(_) => {}
            Heard::NoAnswer(reason) => failed.push(format!("{name}: {reason}")),
        }
    }
    match stopped.len() {
        1 => {
            let (machine, record) = stopped.remove(0);
            Ok(Reply::JobStopped { machine: machine.name, record })
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
pub(crate) fn find(world: &WorldDir, job: &JobId) -> io::Result<Option<PathBuf>> {
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
