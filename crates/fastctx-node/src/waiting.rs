//! `fastctx wait`: waiting until jobs of this machine end (`world-run.md`).
//!
//! A job's end is told once to the resident who started it. When that
//! resident waits for the job itself, the wait takes the end, as an `on`'s
//! follower does, and no word of it follows; a wait that is left before the
//! end hands it back, and the resident is told as usual. A wait for someone
//! else's job takes nothing from them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_platform::is_alive;
use fastctx_types::ipc::Failure;
use fastctx_types::run::JobRecord;
use fastctx_types::{Address, JobId, Reply};
use tokio::sync::broadcast;

use crate::Node;

/// Waits until every job in `jobs` ends. `caller` is the resident whose
/// shell asked, if any.
pub async fn wait(
    node: &Arc<Node>,
    caller: Option<Address>,
    jobs: Vec<JobId>,
    parent: Option<JobId>,
) -> Result<Ready, Failure> {
    let here = node.machine();
    // Listening before the records are read, so no end is missed.
    let mut changes = node.jobs.subscribe();
    let registering = node.clone();
    let wanted = jobs.clone();
    let (paths, readers) = tokio::task::spawn_blocking(move || {
        let _gate = registering.disk.lock();
        let mut paths = Vec::new();
        let mut readers = Vec::new();
        for job in wanted {
            let path = crate::stop::find(&registering.world, &job)
                .map_err(|error| Failure::new(format!("looking for {job}: {error}")))?
                .ok_or_else(|| Failure::new(format!("{here} has no job {job}; `fastctx w {job}` says where it runs, and `on` that machine waits there")))?;
            fastctx_hands::run::record::read(&path).map_err(|error| Failure::new(error.to_string()))?;
            readers.push(crate::job_reader::hold(&registering, &path));
            paths.push(path);
        }
        Ok::<_, Failure>((paths, readers))
    }).await.map_err(|error| Failure::new(error.to_string()))??;
    let mut held = Held { node: node.clone(), jobs: Vec::new(), followers: Vec::new(), _readers: readers, done: false };
    for (job, path) in jobs.iter().zip(&paths) {
        let _delivery = node.job_ends.lock(path).await;
        let record = crate::jobs::read(path).await?;
        if caller.as_ref().is_some_and(|caller| record.owner.resident() == Some(caller)) {
            hold(node, path);
            held.jobs.push((job.clone(), path.clone()));
            let owner = caller.as_ref().expect("checked owner");
            let origin = fastctx_types::on::OnOrigin {
                turn: record.turn.clone(),
                sentinel: None,
                call_id: record.call_id.clone(),
                resident: owner.clone(),
                town: record.town.clone(),
                request: record.on.as_ref().and_then(|on| on.request.clone()),
                follower: None,
                account: None,
            };
            let mut follower = crate::on_delivery::Following::start(node, &origin, &record.machine);
            follower.started(job);
            held.followers.push((job.clone(), follower));
            if node.is_me(&owner.machine) {
                let gate = node.residents.management(&owner.resident)?;
                let _claiming = gate.write().await;
                let directory = node.world.conversation(&owner.resident);
                for pending in crate::inbox::pending(&directory).map_err(|error| Failure::new(error.to_string()))? {
                    if pending.delivery.id == crate::inbox::job_notice_id(&record)
                        && pending.state == fastctx_types::residents::PendingState::Waiting
                    {
                        crate::on_delivery::park(node, &owner.resident, &pending.delivery)
                            .and_then(|_| crate::inbox::withdraw(&directory, &pending.delivery.id))
                            .map_err(|error| Failure::new(error.to_string()))?;
                    }
                }
            }
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
    Ok(Ready { reply: Reply::Waited { records }, held, caller, parent })
}

/// Keep completion claims alive until the IPC client acknowledges its output.
pub(crate) struct Ready {
    pub(crate) reply: Reply,
    held: Held,
    caller: Option<Address>,
    parent: Option<JobId>,
}

impl Ready {
    pub(crate) async fn take(self, delivered: bool) -> Result<(), Failure> {
        let Reply::Waited { records } = &self.reply else { unreachable!("a wait answers with records") };
        let jobs = records
            .iter()
            .map(|record| fastctx_types::run::CompletedJob {
                machine: record.machine.clone(),
                job: record.id.clone(),
                output: record.output.clone(),
            })
            .collect::<Vec<_>>();
        if delivered {
            crate::retention::wait_taken(&self.held.node, self.caller.as_ref(), self.parent.as_ref(), &jobs)
                .map_err(|error| Failure::new(format!("recording waited ends: {error}")))?;
        }
        self.held.take().await
    }
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

/// Followers are counted by the job's record path: short job ids are only
/// unique within one owner's directory.
fn hold(node: &Node, record: &Path) {
    *node.following.lock().expect("never poisoned").entry(record.to_path_buf()).or_default() += 1;
}

fn release(node: &Node, record: &Path) {
    let mut following = node.following.lock().expect("never poisoned");
    if let Some(count) = following.get_mut(record) {
        *count -= 1;
        if *count == 0 {
            following.remove(record);
        }
    }
}

/// The ends this wait holds back from being told: taken when the wait
/// answers, handed back when it is left before.
struct Held {
    node: Arc<Node>,
    jobs: Vec<(JobId, PathBuf)>,
    followers: Vec<(JobId, crate::on_delivery::Following)>,
    _readers: Vec<crate::job_reader::Lease>,
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
                let _delivery = self.node.job_ends.lock(path).await;
                let record = crate::jobs::read(path).await?;
                fastctx_platform::publish(
                    &path.with_extension("wait-taken"),
                    crate::inbox::job_notice_id(&record).as_str().as_bytes(),
                    fastctx_platform::Permissions::Private,
                )
                .map_err(|error| Failure::new(error.to_string()))?;
                for (followed, follower) in &self.followers {
                    if followed == job {
                        follower.taken(job).map_err(|error| Failure::new(error.to_string()))?;
                    }
                }
                if let Some(owner) = record.owner.resident().filter(|owner| self.node.is_me(&owner.machine)) {
                    let gate = self.node.residents.management(&owner.resident)?;
                    let _taking = gate.write().await;
                    let directory = self.node.world.conversation(&owner.resident);
                    crate::inbox::consume(&directory, &crate::inbox::job_notice_id(&record))
                        .map_err(|error| Failure::new(error.to_string()))?;
                } else if let Some(owner) = record.owner.resident() {
                    let end = record.end.as_ref().expect("a wait takes ended jobs");
                    let id = crate::speech::fixed_id("wait-taken", &[crate::inbox::job_notice_id(&record).as_str()]);
                    let notice = fastctx_types::residents::Notice::JobEnded {
                        taken: true,
                        consumed: Vec::new(),
                        follower_unconfirmed: false,
                        job: record.id.clone(),
                        machine: record.machine.clone(),
                        request: record.on.as_ref().and_then(|on| on.request.clone()),
                        follower: None,
                        how: end.how.clone(),
                        status: record.exit.map(|exit| exit.status),
                        at: end.at,
                        output: record.output.clone(),
                        stderr: record.stderr.clone(),
                        last: None,
                        stop: record.stop.as_ref().map(|stop| Box::new(stop.request.clone())),
                        scratch: crate::scratch::jobs::locations(path, &record.machine)
                            .map_err(|error| Failure::new(error.to_string()))?,
                    };
                    let account =
                        record.on.as_ref().and_then(|on| on.account.clone()).or_else(|| record.account.clone());
                    let acting = account
                        .map(|account| fastctx_types::access::Acting::one(account.hub, account.account))
                        .unwrap_or_default();
                    crate::job_receipts::consumed(&self.node, owner, &acting, id, notice)
                        .map_err(|error| Failure::new(error.to_string()))?;
                }
                let owed = self.node.world.job_owner(&self.node.names(), &record.owner).job_owed(job);
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
        for (_, path) in &self.jobs {
            release(&self.node, path);
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
        // Left before the end: whatever ended meanwhile is told as usual,
        // retried as an `on`'s follower that leaves retries it.
        for (_, path) in &self.jobs {
            release(&self.node, path);
            crate::inbox::retry_job(&self.node, path);
        }
    }
}
