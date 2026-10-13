//! Durable intentional stops, including a dispatch whose job id was lost.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_types::access::Purpose;
use fastctx_types::ipc::Failure;
use fastctx_types::on::OnOrigin;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::run::JobOwner;
use fastctx_types::{Address, JobId, MachineName, NoticeId, ResidentId};

use crate::Node;
use crate::again::Again;
use crate::on::LOOK_AGAIN;

/// A stop of a job on another machine that has not reached it yet.
#[derive(serde::Serialize, serde::Deserialize)]
struct PendingStop {
    machine: MachineName,
    /// The hub of `machine`, when it was written as `name.hub`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hub: Option<fastctx_types::HubName>,
    job: Option<JobId>,
    origin: OnOrigin,
}

impl PendingStop {
    fn written(&self) -> crate::reach::Written {
        crate::reach::Written { name: self.machine.clone(), hub: self.hub.clone() }
    }
}

/// Where stops not yet delivered wait, so that a node that restarts still
/// sends them.
fn pending_path(node: &Node, owner: &Address, machine: &MachineName, request: &str) -> PathBuf {
    let id = crate::speech::fixed_id("stop", &[owner.machine.key(), owner.resident.as_str(), machine.key(), request]);
    node.world.owner(&node.names(), owner).book("on-stops").join(format!("{id}.json"))
}

/// The pending stop itself retains suppression if the separate receipt
/// could not yet be written, including during startup recovery.
pub(crate) fn pending_cancellation(
    node: &Node,
    to: &ResidentId,
    machine: &MachineName,
    request: &NoticeId,
) -> std::io::Result<bool> {
    let owner = Address { machine: node.machine(), resident: to.clone() };
    let path = pending_path(node, &owner, machine, request.as_str());
    Ok(path.try_exists()?
        || node.world.path().join("on-stops").join(path.file_name().expect("pending stop name")).try_exists()?)
}

/// Asks `machine` to stop `job`, until it has or the job is gone: the stop
/// was meant, so it is not dropped for a broken connection, nor for this
/// node restarting before the connection came back.
pub(crate) async fn send(node: &Arc<Node>, written: &crate::reach::Written, origin: &OnOrigin, job: Option<&JobId>) {
    let machine = &written.name;
    let pending =
        PendingStop { machine: machine.clone(), hub: written.hub.clone(), job: job.cloned(), origin: origin.clone() };
    let identity = origin
        .request
        .as_ref()
        .map(NoticeId::as_str)
        .or_else(|| job.map(JobId::as_str))
        .expect("a stop names its request or job");
    let path = pending_path(node, &origin.resident, machine, identity);
    let keeping = async {
        let mut again = Again::default();
        loop {
            match keep_stop(&pending, &path).await {
                Ok(()) => return,
                Err(error) => node.log.line(format_args!("keeping {} for after a restart: {error}", path.display())),
            }
            tokio::time::sleep(again.next()).await;
        }
    };
    // Failure of local storage must neither lose the retry nor prevent an
    // otherwise reachable supervisor from being stopped promptly.
    let ((), delivered) = tokio::join!(keeping, deliver_stop(node, &pending));
    match delivered {
        Ok(()) => settle_stop(node, &pending, &path).await,
        Err(error) => node.log.line(format_args!("{} remains pending: {error}", path.display())),
    }
}

async fn keep_stop(pending: &PendingStop, path: &Path) -> std::io::Result<()> {
    let directory = path.parent().expect("pending stop directory").to_path_buf();
    let bytes = serde_json::to_vec(pending).map_err(std::io::Error::other)?;
    let written = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        fastctx_platform::ensure_private_dir(&directory)?;
        fastctx_platform::publish(&written, &bytes, fastctx_platform::Permissions::Private)
    })
    .await
    .map_err(std::io::Error::other)?
}

async fn settle_stop(node: &Node, pending: &PendingStop, path: &Path) {
    let mut again = Again::default();
    loop {
        let settled = crate::on_delivery::cancelled(node, &pending.origin, &pending.machine).and_then(|()| {
            match std::fs::remove_file(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                result => result,
            }
        });
        match settled {
            Ok(()) => return,
            Err(error) => node.log.line(format_args!("settling {}: {error}", path.display())),
        }
        tokio::time::sleep(again.next()).await;
    }
}

async fn deliver_stop(node: &Arc<Node>, pending: &PendingStop) -> Result<(), Failure> {
    if node.is_here(&pending.written()) {
        let mut again = Again::default();
        loop {
            match serve(node, pending.origin.clone(), pending.job.clone()).await {
                Ok(_) => return Ok(()),
                Err(error) => {
                    node.log.line(format_args!("the local stop of {:?} remains pending: {error}", pending.job))
                }
            }
            tokio::time::sleep(again.next()).await;
        }
    }
    let mut again = Again::default();
    let acting = crate::on::acting(node, &pending.origin);
    loop {
        let call = RemoteCall::StopOn { origin: pending.origin.clone(), job: pending.job.clone() };
        match node.call(&acting, &pending.written(), Purpose::Hands, call).await {
            Ok(Ok(RemoteReply::OnDone)) => return Ok(()),
            Ok(Ok(_)) => node.log.line(format_args!(
                "{} answered the stop of {:?} with an unrelated reply",
                pending.machine, pending.job
            )),
            Ok(Err(error)) => {
                node.log.line(format_args!("{} has not stopped {:?}: {error}", pending.machine, pending.job))
            }
            Err(error) => node
                .log
                .line(format_args!("the stop of {:?} on {} remains pending: {error}", pending.job, pending.machine)),
        }
        again.pause(node, &[pending.written()]).await;
    }
}

/// Sends again the stops a node that stopped had not delivered.
pub(crate) async fn resume(node: &Arc<Node>) {
    let recovering = node.clone();
    tokio::spawn(async move {
        let mut again = Again::default();
        loop {
            match resume_local_stops(&recovering) {
                Ok(()) => break,
                Err(error) => recovering.log.line(format_args!("recovering local on stops: {error}")),
            }
            tokio::time::sleep(again.next()).await;
        }
    });
    let recovering = node.clone();
    tokio::spawn(async move {
        let mut again = Again::default();
        loop {
            match resume_pending_stops(&recovering).await {
                Ok(()) => break,
                Err(error) => recovering.log.line(format_args!("recovering outgoing on stops: {error}")),
            }
            tokio::time::sleep(again.next()).await;
        }
    });
}

async fn resume_pending_stops(node: &Arc<Node>) -> std::io::Result<()> {
    let directories = node.world.book_directories("on-stops")?;
    let mut problems = Vec::new();
    for directory in directories {
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(error) => {
                    problems.push(format!("{}: {error}", directory.display()));
                    break;
                }
            };
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if node.on_delivering.lock().expect("never poisoned").contains(&path) {
                continue;
            }
            let read = match tokio::fs::read(&path).await {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                bytes => bytes
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| serde_json::from_slice::<PendingStop>(&bytes).map_err(|error| error.to_string())),
            };
            let pending = match read {
                Ok(pending) => pending,
                Err(error) => {
                    problems.push(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            if !node.on_delivering.lock().expect("never poisoned").insert(path.clone()) {
                continue;
            }
            let node = node.clone();
            tokio::spawn(async move {
                if let Err(error) = deliver_stop(&node, &pending).await {
                    node.log.line(format_args!("{} remains pending: {error}", path.display()));
                    node.on_delivering.lock().expect("never poisoned").remove(&path);
                    return;
                }
                settle_stop(&node, &pending, &path).await;
                node.on_delivering.lock().expect("never poisoned").remove(&path);
            });
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(std::io::Error::other(problems.join("\n"))) }
}

/// Stops an `on` job its sender stopped. The script that sent it stopped
/// it on purpose and learns of that from its own end, so no letter of this
/// end is owed to the resident.
pub(crate) async fn serve(node: &Arc<Node>, origin: OnOrigin, job: Option<JobId>) -> Result<RemoteReply, Failure> {
    let owner = JobOwner::Resident(origin.resident.clone());
    let directory = node.world.job_owner(&node.names(), &owner);
    if let Some(request) = &origin.request {
        fastctx_platform::ensure_private_dir(&directory.jobs())
            .and_then(|()| {
                fastctx_platform::publish(
                    &directory.jobs().join(format!("request-{request}.stopped")),
                    b"",
                    fastctx_platform::Permissions::Private,
                )
            })
            .map_err(|error| Failure::new(format!("keeping the cancelled request: {error}")))?;
    }
    let job = match job {
        Some(job) => Some(job),
        None => {
            let request = origin.request.as_ref().ok_or_else(|| Failure::new("a stop needs a job or a request id"))?;
            let mut found = None;
            let mut unreadable = false;
            for entry in std::fs::read_dir(directory.jobs()).map_err(|error| Failure::new(error.to_string()))? {
                let path = match entry {
                    Ok(entry) => entry.path(),
                    Err(error) => {
                        unreadable = true;
                        node.log.line(format_args!("looking for stopped request {request}: {error}"));
                        continue;
                    }
                };
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let record = match crate::jobs::read(&path).await {
                    Ok(record) => record,
                    Err(error) => {
                        unreadable = true;
                        node.log
                            .line(format_args!("looking for stopped request {request}, {}: {error}", path.display()));
                        continue;
                    }
                };
                if record.on.as_ref().and_then(|on| on.request.as_ref()) == Some(request) {
                    if found.is_some() {
                        return Err(Failure::new("more than one job has this on request id"));
                    }
                    found = Some(record.id);
                }
            }
            if found.is_none() && unreadable {
                return Err(Failure::new("the cancellation is kept, but some job records could not be read"));
            }
            found
        }
    };
    // If it has not prepared a job yet, launch will see the durable stop
    // before spawning its supervisor. If launch passed that check, its
    // record was already published and the scan above found the job.
    let Some(job) = job else { return Ok(RemoteReply::OnDone) };
    let record_path = crate::on::on_job(node, &origin, &job)?;
    let by = format!("{} on {}", origin.resident.resident, origin.resident.machine);
    let request = fastctx_types::run::StopRequest { by, reason: "the on that sent it stopped".to_string() };
    let bytes = serde_json::to_vec(&request).map_err(|error| Failure::new(error.to_string()))?;
    fastctx_platform::publish(
        &record_path.with_extension("on-stopped"),
        &bytes,
        fastctx_platform::Permissions::Private,
    )
    .map_err(|error| Failure::new(format!("keeping the accepted stop: {error}")))?;
    stop_worker(node, &record_path, request);
    loop {
        let record = crate::jobs::read(&record_path).await?;
        if record.end.is_some() {
            stopped(&record)?;
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    Ok(RemoteReply::OnDone)
}

fn stop_worker(node: &Arc<Node>, path: &Path, request: fastctx_types::run::StopRequest) {
    if !node.on_stopping.lock().expect("never poisoned").insert(path.to_path_buf()) {
        return;
    }
    let (node, path) = (node.clone(), path.to_path_buf());
    tokio::spawn(async move {
        let mut again = Again::default();
        loop {
            match crate::jobs::stop(&node, &path, &request.by, &request.reason).await {
                Ok(record) => match stopped(&record) {
                    Ok(()) => break,
                    Err(error) => node.log.line(format_args!("stopping {}: {error}", path.display())),
                },
                Err(error) => node.log.line(format_args!("stopping {}: {error}", path.display())),
            }
            tokio::time::sleep(again.next()).await;
        }
        node.on_stopping.lock().expect("never poisoned").remove(&path);
    });
}

fn stopped(record: &fastctx_types::run::JobRecord) -> Result<(), Failure> {
    use fastctx_types::run::EndedHow;
    match record.end.as_ref().map(|end| &end.how) {
        Some(EndedHow::Finished | EndedHow::Stopped) => Ok(()),
        Some(how) => {
            Err(Failure::new(format!("{} did not establish that its process tree stopped: {how:?}", record.id)))
        }
        None => Err(Failure::new(format!("{} has not ended", record.id))),
    }
}

fn resume_local_stops(node: &Arc<Node>) -> std::io::Result<()> {
    let mut problems = Vec::new();
    for parent in node.world.job_parents()? {
        let owners = match std::fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                problems.push(format!("{}: {error}", parent.display()));
                continue;
            }
        };
        for owner in owners {
            let owner = match owner {
                Ok(owner) => owner,
                Err(error) => {
                    problems.push(format!("{}: {error}", parent.display()));
                    continue;
                }
            };
            let jobs_path = owner.path().join("jobs");
            let jobs = match std::fs::read_dir(&jobs_path) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    problems.push(format!("{}: {error}", jobs_path.display()));
                    continue;
                }
            };
            for entry in jobs {
                let path = match entry {
                    Ok(entry) => entry.path(),
                    Err(error) => {
                        problems.push(format!("{}: {error}", jobs_path.display()));
                        continue;
                    }
                };
                if let Err(error) = resume_local_stop(node, &path) {
                    problems.push(format!("{}: {error}", path.display()));
                }
            }
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(std::io::Error::other(problems.join("\n"))) }
}

fn resume_local_stop(node: &Arc<Node>, path: &Path) -> std::io::Result<()> {
    if path.extension().is_some_and(|extension| extension == "on-stopped") {
        let request = serde_json::from_slice(&std::fs::read(path)?).map_err(std::io::Error::other)?;
        stop_worker(node, &path.with_extension("json"), request);
    } else if path.extension().is_some_and(|extension| extension == "json") {
        let record = fastctx_hands::run::record::read(path)?;
        if let Some(request) = record.on.as_ref().and_then(|on| on.request.as_ref())
            && path.parent().expect("jobs directory").join(format!("request-{request}.stopped")).try_exists()?
            && record.end.is_none()
        {
            stop_worker(
                node,
                path,
                fastctx_types::run::StopRequest {
                    by: record.owner.to_string(),
                    reason: "the on that sent it stopped".to_string(),
                },
            );
        }
    }
    Ok(())
}
