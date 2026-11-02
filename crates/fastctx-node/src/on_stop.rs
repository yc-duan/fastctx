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
use crate::records::items;

/// A stop of a job on another machine that has not reached it yet.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PendingStop {
    machine: MachineName,
    /// The hub of `machine`, when it was written as `name.hub`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hub: Option<fastctx_types::HubName>,
    job: Option<JobId>,
    origin: OnOrigin,
}

pub(crate) fn validate_legacy_pending(bytes: &[u8]) -> std::io::Result<()> {
    let pending: PendingStop = serde_json::from_slice(bytes)?;
    if pending.job.is_none() && pending.origin.request.is_none() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "on stop has neither a job nor a request"));
    }
    Ok(())
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
    Ok(items::exists(&path)?
        || items::exists(&node.world.path().join("on-stops").join(path.file_name().expect("pending stop name")))?)
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
                Err(error) => retry_record(node, &path, format!("keeping the stop: {error}")).await,
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
    let bytes = serde_json::to_vec(pending).map_err(std::io::Error::other)?;
    let written = path.to_path_buf();
    fastctx_platform::task::spawn_blocking(move || items::write(&written, &bytes))
        .await
        .map_err(std::io::Error::other)?
}

async fn settle_stop(node: &Arc<Node>, pending: &PendingStop, path: &Path, impossible: Option<String>) {
    let mut again = Again::default();
    loop {
        let (node, pending, path, impossible) = (node.clone(), pending.clone(), path.to_path_buf(), impossible.clone());
        let settled = fastctx_platform::task::blocking(move || {
            let settled = settle_now(&node, &pending, &path, impossible.as_deref());
            if let Err(error) = &settled {
                let _ = crate::obligations::retry_record(&node, &path, &format!("settling the stop: {error}"));
            }
            settled
        })
        .await;
        if settled.is_ok() {
            return;
        }
        tokio::time::sleep(again.next()).await;
    }
}

fn settle_now(node: &Arc<Node>, pending: &PendingStop, path: &Path, impossible: Option<&str>) -> std::io::Result<()> {
    if impossible.is_none() {
        crate::on_delivery::cancelled(node, &pending.origin, &pending.machine)?;
    }
    let bytes = match items::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if let Some(reason) = impossible {
        crate::obligations::tell_failure(
            node,
            &pending.origin.resident,
            path,
            format!("The stop on {} could not be delivered: {reason}.", pending.machine),
        )?;
    }
    let (outcome, reason) = match impossible {
        Some(reason) => (crate::obligations::Outcome::Impossible, reason),
        None => (crate::obligations::Outcome::Delivered, "the stop was applied or its job is already gone"),
    };
    crate::obligations::finish_record(node, path, &bytes, outcome, reason)?;
    Ok(())
}

/// Notes on the pending stop at `path` why it waits, from a task.
async fn retry_record(node: &Arc<Node>, path: &Path, reason: String) {
    let (node, path) = (node.clone(), path.to_path_buf());
    let _ = fastctx_platform::task::blocking(move || crate::obligations::retry_record(&node, &path, &reason)).await;
}

async fn deliver_stop(node: &Arc<Node>, pending: &PendingStop, path: &Path) -> Result<(), Failure> {
    let mut again = Again::default();
    let acting = crate::on::acting(node, &pending.origin);
    let targets = [pending.written()];
    loop {
        let changed = node.control_letters.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let (asking, acted, written) = (node.clone(), acting.clone(), pending.written());
        if let Some(reason) =
            fastctx_platform::task::blocking(move || crate::obligations::control_impossible(&asking, &acted, &written))
                .await
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
            let (posting, at, acted, written) = (node.clone(), path.to_path_buf(), acting.clone(), pending.written());
            let posted = fastctx_platform::task::blocking(move || {
                crate::control_posts::send(&posting, &at, &bytes, &acted, &written, command)
            })
            .await;
            match posted {
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
                let (keeping, at) = (node.clone(), path.to_path_buf());
                fastctx_platform::task::blocking(move || {
                    crate::obligations::retry_record(&keeping, &at, &error.message)
                })
                .await
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
    fastctx_platform::task::spawn(async move {
        let mut again = Again::default();
        loop {
            let working = recovering.clone();
            let result = fastctx_platform::task::spawn_blocking(move || {
                resume_local_stops(&working).inspect_err(|error| {
                    let _ = crate::obligations::retry(
                        &working,
                        &working.world.path().join("local-stop-recovery"),
                        &error.to_string(),
                    );
                })
            })
            .await;
            match result {
                Ok(Ok(())) => break,
                Ok(Err(_)) => {}
                Err(error) => recovering.log.line(format_args!("recovering local stops: {error}")),
            }
            tokio::time::sleep(again.next()).await;
        }
    });
    let recovering = node.clone();
    fastctx_platform::task::spawn(async move {
        let mut again = Again::default();
        loop {
            match resume_pending_stops(&recovering).await {
                Ok(()) => break,
                Err(error) => {
                    let working = recovering.clone();
                    let _ = fastctx_platform::task::spawn_blocking(move || {
                        crate::obligations::retry(
                            &working,
                            &working.world.path().join("outgoing-stop-recovery"),
                            &error.to_string(),
                        )
                    })
                    .await;
                }
            }
            tokio::time::sleep(again.next()).await;
        }
    });
}

async fn resume_pending_stops(node: &Arc<Node>) -> std::io::Result<()> {
    let working = node.clone();
    let directories = fastctx_platform::task::spawn_blocking(move || working.world.book_directories("on-stops"))
        .await
        .map_err(std::io::Error::other)??;
    let mut problems = Vec::new();
    for directory in directories {
        let mut after = String::new();
        loop {
            let (folder, cursor) = (directory.clone(), after.clone());
            let paths =
                fastctx_platform::task::spawn_blocking(move || items::page(&folder, Some("json"), &cursor, 128))
                    .await
                    .map_err(std::io::Error::other)??;
            let Some(last) = paths.last() else { break };
            after = last.file_name().and_then(|name| name.to_str()).expect("stop identity").to_string();
            for path in paths {
                if node.on_delivering.lock().expect("never poisoned").contains(&path) {
                    continue;
                }
                let reading = path.clone();
                let pending = fastctx_platform::task::spawn_blocking(move || items::get::<PendingStop>(&reading))
                    .await
                    .map_err(std::io::Error::other)?;
                let pending = match pending {
                    Ok(Some(pending)) => pending,
                    Ok(None) => continue,
                    Err(error) => {
                        problems.push(format!("{}: {error}", path.display()));
                        continue;
                    }
                };
                if !node.on_delivering.lock().expect("never poisoned").insert(path.clone()) {
                    continue;
                }
                let node = node.clone();
                fastctx_platform::task::spawn(async move {
                    match deliver_stop(&node, &pending, &path).await {
                        Ok(()) => settle_stop(&node, &pending, &path, None).await,
                        Err(error) if crate::obligations::permanent(&error) => {
                            settle_stop(&node, &pending, &path, Some(error.message)).await
                        }
                        Err(error) => node.log.line(format_args!("{} remains pending: {error}", path.display())),
                    }
                    node.on_delivering.lock().expect("never poisoned").remove(&path);
                });
            }
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(std::io::Error::other(problems.join("\n"))) }
}

/// Stops an `on` job its sender stopped. The script that sent it stopped
/// it on purpose and learns of that from its own end, so no letter of this
/// end is owed to the resident.
pub(crate) async fn serve(node: &Arc<Node>, origin: OnOrigin, job: Option<JobId>) -> Result<RemoteReply, Failure> {
    let accepting = node.clone();
    let accepted = fastctx_platform::task::blocking(move || accept(&accepting, &origin, job)).await?;
    let Some((record_path, request)) = accepted else { return Ok(RemoteReply::OnDone) };
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

/// Keeps the cancellation of `origin`'s request, finds its job and keeps the
/// accepted stop beside the job's record; says where that record is and the
/// stop, or nothing when there is no job to stop.
fn accept(
    node: &Arc<Node>,
    origin: &OnOrigin,
    job: Option<JobId>,
) -> Result<Option<(PathBuf, fastctx_types::run::StopRequest)>, Failure> {
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
            for entry in
                fastctx_platform::disk::read_dir(directory.jobs()).map_err(|error| Failure::new(error.to_string()))?
            {
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
                let record = match crate::jobs::read_now(&path) {
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
    let Some(job) = job else { return Ok(None) };
    let record_path = match crate::on::on_job(node, origin, &job) {
        Ok(path) => path,
        Err(error) if error.code == Some(fastctx_types::ipc::FailureCode::OutputNotRetained) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let by = format!("{} on {}", origin.resident.resident, origin.resident.machine);
    let request = fastctx_types::run::StopRequest { by, reason: "the on that sent it stopped".to_string() };
    let bytes = serde_json::to_vec(&request).map_err(|error| Failure::new(error.to_string()))?;
    crate::job_metadata::put_raw(&record_path.with_extension("on-stopped"), &bytes)
        .map_err(|error| Failure::new(format!("keeping the accepted stop: {error}")))?;
    Ok(Some((record_path, request)))
}

fn stop_worker(node: &Arc<Node>, path: &Path, request: fastctx_types::run::StopRequest) {
    if !node.on_stopping.lock().expect("never poisoned").insert(path.to_path_buf()) {
        return;
    }
    let (node, path) = (node.clone(), path.to_path_buf());
    fastctx_platform::task::spawn(async move {
        let mut again = Again::default();
        loop {
            let looking = path.clone();
            if fastctx_platform::task::blocking(move || fastctx_platform::disk::exists(&looking))
                .await
                .is_ok_and(|exists| !exists)
            {
                break;
            }
            let failed = match crate::jobs::stop(&node, &path, &request.by, &request.reason).await {
                Ok(record) => stopped(&record).err(),
                Err(error) => Some(error),
            };
            let Some(error) = failed else { break };
            let permanent = crate::obligations::permanent(&error);
            let (keeping, stopped_at) = (node.clone(), path.with_extension("on-stopped"));
            let _ = fastctx_platform::task::blocking(move || {
                crate::obligations::retry_job(&keeping, &stopped_at, &error.message)
            })
            .await;
            if permanent {
                break;
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
        let owners = match fastctx_platform::disk::read_dir(&parent) {
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
            let jobs = match fastctx_platform::disk::read_dir(&jobs_path) {
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
    if path.extension().is_some_and(|extension| extension == "json") {
        if let Some(request) = crate::job_metadata::get(&path.with_extension("on-stopped"))? {
            stop_worker(node, path, request);
            return Ok(());
        }
        let record = fastctx_hands::run::record::read(path)?;
        if let Some(request) = record.on.as_ref().and_then(|on| on.request.as_ref())
            && fastctx_platform::disk::exists(
                path.parent().expect("jobs directory").join(format!("request-{request}.stopped")),
            )?
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
