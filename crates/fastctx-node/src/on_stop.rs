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
                Err(error) => {
                    let _ = crate::obligations::retry(node, &path, &format!("keeping the stop: {error}"));
                }
            }
            tokio::time::sleep(again.next()).await;
        }
    };
    // Failure of local storage must neither lose the retry nor prevent an
    // otherwise reachable supervisor from being stopped promptly.
    let ((), delivered) = tokio::join!(keeping, deliver_stop(node, &pending, &path));
    match delivered {
        Ok(()) => settle_stop(node, &pending, &path, None).await,
        Err(error) if crate::obligations::permanent(&error) => {
            settle_stop(node, &pending, &path, Some(error.message)).await
        }
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

async fn settle_stop(node: &Arc<Node>, pending: &PendingStop, path: &Path, impossible: Option<String>) {
    let mut again = Again::default();
    loop {
        let settled = (|| -> std::io::Result<()> {
            if impossible.is_none() {
                crate::on_delivery::cancelled(node, &pending.origin, &pending.machine)?;
            }
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            if let Some(reason) = &impossible {
                crate::obligations::tell_failure(
                    node,
                    &pending.origin.resident,
                    path,
                    format!("The stop on {} could not be delivered: {reason}.", pending.machine),
                )?;
            }
            let (outcome, reason) = match &impossible {
                Some(reason) => (crate::obligations::Outcome::Impossible, reason.as_str()),
                None => (crate::obligations::Outcome::Delivered, "the stop was applied or its job is already gone"),
            };
            crate::obligations::finish(node, path, &bytes, outcome, reason)?;
            Ok(())
        })();
        match settled {
            Ok(()) => return,
            Err(error) => {
                let _ = crate::obligations::retry(node, path, &format!("settling the stop: {error}"));
            }
        }
        tokio::time::sleep(again.next()).await;
    }
}

async fn deliver_stop(node: &Arc<Node>, pending: &PendingStop, path: &Path) -> Result<(), Failure> {
    let mut again = Again::default();
    let acting = crate::on::acting(node, &pending.origin);
    let targets = [pending.written()];
    loop {
        let changed = node.control_letters.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if let Some(reason) = crate::obligations::control_impossible(node, &acting, &pending.written())
            .map_err(|error| Failure::new(error.to_string()))?
        {
            return Err(Failure::refused(reason));
        }
        let result = if node.is_here(&pending.written()) {
            serve(node, pending.origin.clone(), pending.job.clone()).await.map(|_| ())
        } else if crate::facts::control_letters(node, &pending.written()) {
            let bytes = serde_json::to_vec(pending).map_err(|error| Failure::new(error.to_string()))?;
            let command = fastctx_types::delivery_control::ControlLetter::StopOn {
                origin: pending.origin.clone(),
                job: pending.job.clone(),
            };
            match crate::control_posts::send(node, path, &bytes, &acting, &pending.written(), command) {
                Ok(Some(result)) => result,
                Ok(None) => {
                    tokio::select! {
                        () = again.pause(node, &targets) => {}
                        () = &mut changed => {}
                    }
                    continue;
                }
                Err(error) => Err(Failure::new(error.to_string())),
            }
        } else {
            let call = RemoteCall::StopOn { origin: pending.origin.clone(), job: pending.job.clone() };
            match node.call(&acting, &pending.written(), Purpose::Hands, call).await {
                Ok(Ok(RemoteReply::OnDone)) => Ok(()),
                Ok(Ok(_)) => Err(Failure::new("the stop received an unrelated reply")),
                Ok(Err(error)) => Err(error),
                Err(error) => Err(Failure::new(error.to_string())),
            }
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error) if crate::obligations::permanent(&error) => return Err(error),
            Err(error) => {
                crate::obligations::retry(node, path, &error.message)
                    .map_err(|error| Failure::new(error.to_string()))?;
            }
        }
        tokio::select! {
            () = again.pause(node, &targets) => {}
            () = &mut changed => {}
        }
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
                Err(error) => {
                    let _ = crate::obligations::retry(
                        &recovering,
                        &recovering.world.path().join("local-stop-recovery"),
                        &error.to_string(),
                    );
                }
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
                Err(error) => {
                    let _ = crate::obligations::retry(
                        &recovering,
                        &recovering.world.path().join("outgoing-stop-recovery"),
                        &error.to_string(),
                    );
                }
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
                match deliver_stop(&node, &pending, &path).await {
                    Ok(()) => settle_stop(&node, &pending, &path, None).await,
                    Err(error) if crate::obligations::permanent(&error) => {
                        settle_stop(&node, &pending, &path, Some(error.message)).await;
                    }
                    Err(error) => node.log.line(format_args!("{} remains pending: {error}", path.display())),
                }
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
    let record_path = match crate::on::on_job(node, &origin, &job) {
        Ok(path) => path,
        Err(error) if error.code == Some(fastctx_types::ipc::FailureCode::OutputNotRetained) => {
            return Ok(RemoteReply::OnDone);
        }
        Err(error) => return Err(error),
    };
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
            if path.try_exists().is_ok_and(|exists| !exists) {
                break;
            }
            match crate::jobs::stop(&node, &path, &request.by, &request.reason).await {
                Ok(record) => match stopped(&record) {
                    Ok(()) => break,
                    Err(error) => {
                        let _ = crate::obligations::retry(&node, &path.with_extension("on-stopped"), &error.message);
                        if crate::obligations::permanent(&error) {
                            break;
                        }
                    }
                },
                Err(error) => {
                    let _ = crate::obligations::retry(&node, &path.with_extension("on-stopped"), &error.message);
                }
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
        Some(how) => Err(Failure::refused(format!(
            "{} already ended without a confirmed stop ({how:?}); it is no longer a live stopping target",
            record.id
        ))),
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
