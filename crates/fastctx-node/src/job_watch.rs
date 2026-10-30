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
pub(crate) fn restore(node: &Arc<Node>, path: &Path, record: &JobRecord) -> std::io::Result<()> {
    if record.call_id.is_some() {
        crate::run_facts::update_job_with(node, path, crate::run_facts::writes(path)?, crate::run_facts::steps(path)?)?;
    }
    if let Some(identity) = &record.supervisor {
        start(node, path, Some(identity.clone()));
    } else if let Some(program) = &record.program
        && program.starter != fastctx_platform::current_identity()?
    {
        start(node, path, None);
    }
    Ok(())
}

/// Releases transferred bytes after the final reader leaves. Metadata and
/// scratch await the enclosing answer's host receipt.
pub(crate) fn copied(node: &Arc<Node>, path: &Path) {
    let _gate = node.disk.lock();
    if let Err(error) =
        fastctx_platform::publish(&path.with_extension("copied"), b"", fastctx_platform::Permissions::Private)
    {
        node.log.line(format_args!("recording transferred command output: {error}"));
        return;
    }
    {
        let mut watched = node.job_watches.0.lock().expect("never poisoned");
        if watched.watching.contains(path) {
            watched.forgetting.insert(path.to_path_buf());
            return;
        }
    }
    remove(node, path.to_path_buf());
}

/// A maintenance pass already runs off the control loop. Remove settled
/// jobs sequentially rather than queueing one blocking task per old record.
pub(crate) fn collect(node: &Node, path: &Path) -> std::io::Result<()> {
    let mut watched = node.job_watches.0.lock().expect("never poisoned");
    if watched.watching.contains(path) {
        watched.forgetting.insert(path.to_path_buf());
        return Ok(());
    }
    drop(watched);
    crate::jobs::remove(path)
}

fn remove(node: &Arc<Node>, path: PathBuf) {
    node.disk_due.lock().expect("never poisoned").insert(path);
    node.disk_changed.notify_one();
}

/// Without an identity, the supervisor lock witnesses whether an exited
/// launch or an earlier node's unfinished startup still has a supervisor.
pub(crate) fn start(node: &Arc<Node>, path: &Path, identity: Option<ProcessIdentity>) {
    if !node.job_watches.0.lock().expect("never poisoned").watching.insert(path.to_path_buf()) {
        return;
    }
    let (node, path) = (node.clone(), path.to_path_buf());
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
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
                    tokio::time::sleep(again.next()).await;
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
