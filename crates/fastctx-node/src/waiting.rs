//! `fastctx wait`: waiting until jobs of this machine end (`world-run.md`).
//!
//! A job's end is told once to the resident who started it. When that
//! resident waits for the job itself, the wait takes the end, as an `on`'s
//! follower does, and no word of it follows; a wait that is left before the
//! end hands it back, and the resident is told as usual. A wait for someone
//! else's job takes nothing from them.

use std::path::PathBuf;
use std::sync::Arc;

use fastctx_platform::is_alive;
use fastctx_types::ipc::Failure;
use fastctx_types::run::JobRecord;
use fastctx_types::{Address, JobId, Reply};
use tokio::sync::broadcast;

use crate::Node;

/// Waits until every job in `jobs` ends. `caller` is the resident whose
/// shell asked, if any.
pub async fn wait(node: &Arc<Node>, caller: Option<Address>, jobs: Vec<JobId>) -> Result<Reply, Failure> {
    let here = node.machine();
    // Listening before the records are read, so no end is missed.
    let mut changes = node.jobs.subscribe();
    let mut paths = Vec::new();
    for job in &jobs {
        let (world, wanted) = (node.world.clone(), job.clone());
        let path = tokio::task::spawn_blocking(move || crate::stop::find(&world, &wanted))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("looking for {job}: {error}")))?
            .ok_or_else(|| {
                Failure::new(format!(
                    "{here} has no job {job}; `fastctx w {job}` says where it runs, and `on` that machine waits there"
                ))
            })?;
        paths.push(path);
    }
    let mut held = Held { node: node.clone(), jobs: Vec::new(), done: false };
    for (job, path) in jobs.iter().zip(&paths) {
        let record = crate::jobs::read(path).await?;
        if caller.as_ref().is_some_and(|caller| record.owner.resident() == Some(caller)) {
            hold(node, job);
            held.jobs.push((job.clone(), path.clone()));
        }
    }
    let records = loop {
        let mut records = Vec::new();
        for path in &paths {
            records.push(ended_or_gone(path).await?);
        }
        if records.iter().all(|record| record.end.is_some()) {
            break records;
        }
        match changes.recv().await {
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => unreachable!("the node holds the sender"),
        }
    };
    held.take().await?;
    Ok(Reply::Waited { records })
}

/// A job's record, marked interrupted if its supervisor is gone without
/// having finished it: nothing would ever end it then.
async fn ended_or_gone(path: &std::path::Path) -> Result<JobRecord, Failure> {
    let record = crate::jobs::read(path).await?;
    if record.end.is_some() {
        return Ok(record);
    }
    let gone = match record.supervisor.clone() {
        // A supervisor that cannot be checked is not taken for gone.
        Some(identity) => {
            tokio::task::spawn_blocking(move || !is_alive(&identity).unwrap_or(true)).await.unwrap_or(false)
        }
        None => false,
    };
    if gone {
        return crate::jobs::interrupt(path, "its supervisor ended without finishing the record").await;
    }
    Ok(record)
}

fn hold(node: &Node, job: &JobId) {
    *node.following.lock().expect("never poisoned").entry(job.clone()).or_default() += 1;
}

fn release(node: &Node, job: &JobId) {
    let mut following = node.following.lock().expect("never poisoned");
    if let Some(count) = following.get_mut(job) {
        *count -= 1;
        if *count == 0 {
            following.remove(job);
        }
    }
}

/// The ends this wait holds back from being told: taken when the wait
/// answers, handed back when it is left before.
struct Held {
    node: Arc<Node>,
    jobs: Vec<(JobId, PathBuf)>,
    done: bool,
}

impl Held {
    /// The wait answers with these ends: they are no longer owed. Each is
    /// still held while its debt is cleared, so a change arriving meanwhile
    /// cannot tell it too.
    async fn take(mut self) -> Result<(), Failure> {
        let mut failed = None;
        for (job, path) in &self.jobs {
            let cleared = async {
                let record = crate::jobs::read(path).await?;
                let owed =
                    self.node.world.job_owner(&self.node.machine(), record.town.as_ref(), &record.owner).job_owed(job);
                match tokio::fs::remove_file(&owed).await {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        Err(Failure::new(format!("{}: {error}", owed.display())))
                    }
                    _ => Ok(()),
                }
            }
            .await;
            if let Err(failure) = cleared {
                failed.get_or_insert(failure);
            }
        }
        for (job, _) in &self.jobs {
            release(&self.node, job);
        }
        self.done = true;
        failed.map_or(Ok(()), Err)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        for (job, _) in &self.jobs {
            release(&self.node, job);
        }
        // Left before the end: whatever ended meanwhile is told as usual.
        let (node, paths) = (self.node.clone(), self.jobs.iter().map(|(_, path)| path.clone()).collect::<Vec<_>>());
        tokio::spawn(async move {
            for path in paths {
                let told = match crate::jobs::read(&path).await {
                    Ok(record) => crate::inbox::tell_if_ended(&node, &record).await.map_err(|error| error.to_string()),
                    Err(failure) => Err(failure.message),
                };
                if let Err(error) = told {
                    node.log.line(format_args!("telling the end of a job a wait left: {error}"));
                }
            }
        });
    }
}
