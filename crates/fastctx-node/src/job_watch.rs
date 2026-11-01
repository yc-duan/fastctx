//! A supervisor can exit after failing to publish its last record. Its
//! process identity still establishes that supervision ended; reconcile
//! that fact without waiting for the node itself to restart.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
pub(crate) async fn restore(node: &Arc<Node>, path: &Path, record: &JobRecord) -> std::io::Result<()> {
    if record.call_id.is_some() {
        let (node, path) = (node.clone(), path.to_path_buf());
        fastctx_platform::task::spawn_blocking(move || crate::run_facts::update_job(&node, &path))
            .await
            .map_err(std::io::Error::other)??;
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
    if let Err(error) = crate::job_metadata::put_raw(&path.with_extension("copied"), b"") {
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
    match fastctx_hands::run::record::read(path) {
        Ok(record) => match crate::activity_book::consumed(node, &record) {
            Ok(true) => {}
            result => {
                node.disk_due.lock().expect("never poisoned").insert(path.to_path_buf());
                result?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "the activity book still needs this job",
                ));
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    crate::jobs::remove(path)
}

fn remove(node: &Arc<Node>, path: PathBuf) {
    node.disk_due.lock().expect("never poisoned").insert(path);
    node.disk_changed.notify_one();
}

/// How often a supervisor is looked at where the system cannot say when a
/// process ends.
const LOOKED_AT: Duration = Duration::from_secs(1);

/// The longest between looks at the record's lock of a supervisor whose
/// identity is unknown.
const LOCK_AT_MOST: Duration = Duration::from_secs(5);

/// Without an identity, the supervisor lock witnesses whether an exited
/// launch or an earlier node's unfinished startup still has a supervisor.
/// With one, the watch waits on the supervisor's process itself, and wakes
/// when it ends: a quiet job costs nothing while it runs.
pub(crate) fn start(node: &Arc<Node>, path: &Path, identity: Option<ProcessIdentity>) {
    if !node.job_watches.0.lock().expect("never poisoned").watching.insert(path.to_path_buf()) {
        return;
    }
    let (node, path) = (node.clone(), path.to_path_buf());
    fastctx_platform::task::spawn(async move {
        let mut identity = identity;
        let mut again = crate::again::Again::default();
        let mut lock_wait = crate::on::LOOK_AGAIN;
        loop {
            if let Some(supervisor) = &identity {
                ended(supervisor).await;
            }
            match crate::jobs::interrupt(&path, "its supervisor ended without finishing the record").await {
                Ok(record) if record.end.is_some() => {
                    match node.jobs.changed(&node, record.id, record.owner, record.town).await {
                        Ok(()) => break,
                        Err(error) => {
                            node.log.line(format_args!("recovering {} after its supervisor: {error}", path.display()));
                            tokio::time::sleep(again.next()).await;
                        }
                    }
                }
                // A supervisor holds the record after all: the one it names
                // is waited for now, or, unknown, its lock is tried again.
                Ok(record) => match record.supervisor {
                    Some(named) if identity.as_ref() != Some(&named) => identity = Some(named),
                    _ => {
                        identity = None;
                        tokio::time::sleep(lock_wait).await;
                        lock_wait = (lock_wait * 2).min(LOCK_AT_MOST);
                    }
                },
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

/// Resolves once the process `supervisor` has ended.
async fn ended(supervisor: &ProcessIdentity) {
    if fastctx_platform::host_process::exited(supervisor).await.is_ok() {
        return;
    }
    // Where the system cannot say when a process ends, it is looked at.
    while fastctx_platform::is_alive(supervisor).unwrap_or(false) {
        tokio::time::sleep(LOOKED_AT).await;
    }
}
