//! A supervisor can exit after failing to publish its last record. Its
//! process identity still establishes that supervision ended; reconcile
//! that fact without waiting for the node itself to restart.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_types::ipc::Failure;
use fastctx_types::{ProcessIdentity, run::JobRecord};

use crate::Node;

#[derive(Default)]
pub(crate) struct Watches(Mutex<Watched>);

#[derive(Default)]
struct Watched {
    /// Records whose supervisor is being watched.
    watching: HashSet<PathBuf>,
    /// Watched jobs to remove once their watch ends: a watch still running
    /// takes the record's lock and reads the record.
    forgetting: HashSet<PathBuf>,
}

/// Restores observation of jobs whose launching node has already exited.
pub(crate) fn restore(node: &Arc<Node>, record: &JobRecord) -> std::io::Result<()> {
    let path = node.world.job_owner(&node.machine(), record.town.as_ref(), &record.owner).job_record(&record.id);
    if let Some(identity) = &record.supervisor {
        start(node, &path, Some(identity.clone()));
    } else if let Some(program) = &record.program
        && program.starter != fastctx_platform::current_identity()?
    {
        start(node, &path, None);
    }
    Ok(())
}

/// Removes an ended job's files now, or when its watch ends.
pub(crate) fn forget(node: &Arc<Node>, path: &Path) {
    {
        let mut watched = node.job_watches.0.lock().expect("never poisoned");
        if watched.watching.contains(path) {
            watched.forgetting.insert(path.to_path_buf());
            return;
        }
    }
    remove(node, path.to_path_buf());
}

fn remove(node: &Arc<Node>, path: PathBuf) {
    let node = node.clone();
    tokio::spawn(async move {
        let removed = tokio::task::spawn_blocking(move || crate::jobs::remove(&path)).await;
        match removed {
            Ok(Ok(())) => {}
            Ok(Err(error)) => node.log.line(format_args!("removing a job shown whole: {error}")),
            Err(error) => node.log.line(format_args!("removing a job shown whole: {error}")),
        }
    });
}

/// Without an identity, the supervisor lock witnesses whether an exited
/// launch or an earlier node's unfinished startup still has a supervisor.
pub(crate) fn start(node: &Arc<Node>, path: &Path, identity: Option<ProcessIdentity>) {
    if !node.job_watches.0.lock().expect("never poisoned").watching.insert(path.to_path_buf()) {
        return;
    }
    let (node, path) = (node.clone(), path.to_path_buf());
    tokio::spawn(async move {
        let mut delay = crate::on::Backoff::default();
        loop {
            let result: Result<bool, Failure> = async {
                let alive = identity
                    .as_ref()
                    .map(fastctx_platform::is_alive)
                    .transpose()
                    .map_err(|error| Failure::new(format!("observing the job's supervisor: {error}")))?
                    .unwrap_or(false);
                if alive {
                    return Ok(false);
                }
                let record = crate::jobs::interrupt(&path, "its supervisor ended without finishing the record").await?;
                if record.end.is_none() {
                    return Ok(false);
                }
                node.jobs.changed(&node, record.id, record.owner, record.town).await?;
                Ok(true)
            }
            .await;
            match result {
                Ok(true) => break,
                // This is the existing observation cadence for a quiet job,
                // not a deadline for its execution or for recovery.
                Ok(false) => tokio::time::sleep(crate::on::LOOK_AGAIN).await,
                Err(error) => {
                    node.log.line(format_args!("recovering {} after its supervisor: {error}", path.display()));
                    tokio::time::sleep(delay.next()).await;
                }
            }
        }
        let forgotten = {
            let mut watched = node.job_watches.0.lock().expect("never poisoned");
            watched.watching.remove(&path);
            watched.forgetting.remove(&path)
        };
        if forgotten {
            remove(&node, path);
        }
    });
}
