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
pub(crate) struct Watches(Mutex<HashSet<PathBuf>>);

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

/// Without an identity, the supervisor lock witnesses whether an exited
/// launch or an earlier node's unfinished startup still has a supervisor.
pub(crate) fn start(node: &Arc<Node>, path: &Path, identity: Option<ProcessIdentity>) {
    if !node.job_watches.0.lock().expect("never poisoned").insert(path.to_path_buf()) {
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
        node.job_watches.0.lock().expect("never poisoned").remove(&path);
    });
}
