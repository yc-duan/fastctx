//! A turn stop addresses one dispatch, never a conversation's next turn.
//! The receiver keeps a cancellation before acknowledging it, including when
//! its run has not arrived yet. Old peers can be stopped by a recovered job id.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::access::{Acting, HubAccount, Purpose, Written};
use fastctx_types::delivery_control::ControlLetter;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::run::{Ended, EndedHow, StopRequest, StopRequested};
use fastctx_types::{Address, JobId, NoticeId};
use serde::{Deserialize, Serialize};

use crate::Node;
use crate::obligations::Outcome;

#[derive(Serialize, Deserialize)]
struct Pending {
    owner: Address,
    acting: Acting,
    destination: Written,
    dispatch: NoticeId,
    addressed: bool,
}

#[derive(Serialize, Deserialize)]
struct Incoming {
    job: JobId,
    scope: HubAccount,
}

fn pending_path(node: &Node, owner: &Address, dispatch: &NoticeId) -> PathBuf {
    node.world.owner(&node.names(), owner).book("run-stops").join(format!("{dispatch}.json"))
}

fn incoming_path(node: &Node, owner: &Address, dispatch: &NoticeId) -> PathBuf {
    node.world.owner(&node.names(), owner).book("run-requests").join(format!("{dispatch}.json"))
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    fastctx_install::renewal::read_json(path)
}

fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    ensure_private_dir(path.parent().expect("request directory"))?;
    fastctx_install::renewal::write_json(path, value)
}

/// Persists before the run's caller can return from observing the interruption.
pub(crate) async fn keep(
    node: &Arc<Node>,
    owner: &Address,
    acting: &Acting,
    destination: &Written,
    dispatch: &NoticeId,
) -> io::Result<()> {
    let path = pending_path(node, owner, dispatch);
    let pending = Pending {
        owner: owner.clone(),
        acting: acting.clone(),
        destination: destination.clone(),
        dispatch: dispatch.clone(),
        addressed: crate::facts::run_stops(node, destination),
    };
    let mut again = crate::again::Again::default();
    loop {
        match write(&path, &pending) {
            Ok(()) => break,
            Err(error) => {
                let _ = crate::obligations::retry(node, &path, &format!("keeping the turn's stop: {error}"));
            }
        }
        tokio::time::sleep(again.next()).await;
    }
    let addressed = pending.addressed;
    start(node, path, false);
    if !addressed {
        // An older receiver cannot address the dispatch. While the run still
        // waits there, the conversation-wide interruption it knows stops it at
        // once. It goes once and is never replayed, as a late one could stop a
        // later turn; the job named in the run's answer completes the stop.
        let (node, acting, destination) = (node.clone(), acting.clone(), destination.clone());
        let stop = RemoteCall::TurnInterrupted { resident: owner.resident.clone(), dispatch: None };
        tokio::spawn(async move {
            if let Err(failure) = node.call(&acting, &destination, Purpose::Hands, stop).await {
                node.log.line(format_args!("telling {} that the turn was interrupted: {failure}", destination.name));
            }
        });
    }
    Ok(())
}

/// What the stopped run's own call answered, which an older receiver's stop
/// goes by.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Answered {
    /// The run started this job there.
    Job(JobId),
    /// The run never started there: nothing is left to stop.
    NotRun,
    /// Its answer was lost, or named no job.
    Unknown,
}

/// Kept apart from the immutable stop body, whose digest binds its mailed reply.
pub(crate) fn answered(node: &Node, owner: &Address, dispatch: &NoticeId, answered: Answered) -> io::Result<()> {
    let path = pending_path(node, owner, dispatch);
    if path.try_exists()? {
        write(&path.with_extension("answer"), &answered)?;
        node.control_letters.changed.notify_waiters();
    }
    Ok(())
}

pub(crate) fn resume(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        loop {
            let result = (|| -> io::Result<()> {
                for directory in node.world.book_directories("run-stops")? {
                    for path in crate::retention::entries(&directory)? {
                        if path.extension().is_some_and(|ext| ext == "json") {
                            start(&node, path, true);
                        }
                    }
                }
                Ok(())
            })();
            match result {
                Ok(()) => return,
                Err(error) => {
                    let _ = crate::obligations::retry(
                        &node,
                        &node.world.path().join("run-stop-recovery"),
                        &error.to_string(),
                    );
                }
            }
            tokio::time::sleep(again.next()).await;
        }
    });
}

fn start(node: &Arc<Node>, path: PathBuf, recovered: bool) {
    if !node.on_delivering.lock().expect("never poisoned").insert(path.clone()) {
        return;
    }
    let node = node.clone();
    tokio::spawn(async move {
        let _doing = fastctx_platform::doing::doing("delivering a turn's addressed stop");
        let mut again = crate::again::Again::default();
        loop {
            let changed = node.control_letters.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let mut ways = node.ways.subscribe();
            match attempt(&node, &path, recovered).await {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => {
                    let _ = crate::obligations::retry(&node, &path, &error.to_string());
                }
            }
            tokio::select! {
                () = tokio::time::sleep(again.next()) => {},
                () = &mut changed => {},
                _ = ways.changed() => {},
            }
        }
        node.on_delivering.lock().expect("never poisoned").remove(&path);
    });
}

async fn attempt(node: &Arc<Node>, path: &Path, recovered: bool) -> io::Result<bool> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    let pending: Pending = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let result = if let Some(reason) =
        crate::obligations::control_impossible(node, &pending.acting, &pending.destination)?
    {
        Err(Failure::refused(reason))
    } else if pending.addressed {
        let command = ControlLetter::StopRun { owner: pending.owner.clone(), dispatch: pending.dispatch.clone() };
        let Some(result) =
            crate::control_posts::send(node, path, &bytes, &pending.acting, &pending.destination, command)?
        else {
            return Ok(false);
        };
        result
    } else {
        // A legacy resident-wide interruption cannot be replayed safely: a
        // later turn might already be waiting there. Its concrete job is safe.
        let answer = path.with_extension("answer");
        let answered: Answered = match std::fs::read(&answer) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound && !recovered => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Answered::Unknown,
            Err(error) => return Err(error),
        };
        match answered {
            Answered::Job(job) => {
                let call = RemoteCall::StopJob { job, by: fastctx_types::residents::STOPPED_BY_USER.into() };
                match node.call(&pending.acting, &pending.destination, Purpose::Hands, call).await {
                    Ok(Ok(RemoteReply::JobStopped { .. })) => Ok(()),
                    Ok(Ok(_)) => Err(Failure::new("the stop received an unrelated reply")),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(Failure::new(error.to_string())),
                }
            }
            Answered::NotRun => {
                crate::obligations::finish(node, path, &bytes, Outcome::Superseded, "the run never started there")?;
                return Ok(true);
            }
            Answered::Unknown => Err(Failure::refused(
                "this receiver cannot address a turn stop and its job identity was not recovered; upgrade it to deliver stops after a lost answer",
            )),
        }
    };
    match result {
        Ok(()) => {
            crate::obligations::finish(
                node,
                path,
                &bytes,
                Outcome::Delivered,
                "the addressed stop was accepted or its job already ended",
            )?;
        }
        Err(error) if crate::obligations::permanent(&error) => {
            crate::obligations::tell_failure(
                node,
                &pending.owner,
                path,
                format!("The stop on {} could not be delivered: {}.", pending.destination.name, error.message),
            )?;
            crate::obligations::finish(node, path, &bytes, Outcome::Impossible, &error.message)?;
        }
        Err(error) => {
            crate::obligations::retry(node, path, &error.message)?;
            return Ok(false);
        }
    }
    Ok(true)
}

/// Called with the jobs' disk gate held, after the job record is durable and
/// before its supervisor starts. The same gate serializes cancellation below.
pub(crate) fn register(node: &Arc<Node>, path: &Path, dispatch: &NoticeId) -> Result<(), Failure> {
    let result = (|| -> io::Result<bool> {
        let mut record = fastctx_hands::run::record::read(path)?;
        let owner = record.owner.resident().ok_or_else(|| io::Error::other("a dispatch needs a resident"))?;
        let scope = record.account.clone().ok_or_else(|| io::Error::other("a dispatch needs its account"))?;
        let incoming = incoming_path(node, owner, dispatch);
        if read::<Incoming>(&incoming)?.is_some() {
            return Err(io::Error::other(
                "this dispatch was already accepted; its previous execution was not repeated",
            ));
        }
        write(&incoming, &Incoming { job: record.id.clone(), scope: scope.clone() })?;
        publish(&path.with_extension("run-request"), dispatch.as_str().as_bytes(), Permissions::Private)?;
        if let Some(cancelled) = read::<HubAccount>(&incoming.with_extension("stopped"))? {
            if cancelled != scope {
                return Err(io::Error::other("the dispatch cancellation belongs to another account"));
            }
            let at = jiff::Timestamp::now();
            record.stop = Some(StopRequested { at, request: stop_request() });
            record.end = Some(Ended { at, how: EndedHow::Stopped });
            fastctx_hands::run::record::write(path, &record)?;
            return Ok(true);
        }
        Ok(false)
    })();
    match result {
        Ok(false) => Ok(()),
        Ok(true) => {
            crate::inbox::retry_job(node, path);
            Err(Failure::not_run("the turn was stopped before this dispatch's supervisor started"))
        }
        Err(error) => Err(Failure::new(format!("registering the run dispatch: {error}"))),
    }
}

fn stop_request() -> StopRequest {
    StopRequest {
        by: fastctx_types::residents::STOPPED_BY_USER.into(),
        reason: "the turn that was waiting for it was interrupted".into(),
    }
}

/// A cancellation without a job is a durable promise to refuse its later launch.
/// One with a job waits for that exact job to end before acknowledging success.
pub(crate) async fn serve(
    node: &Arc<Node>,
    owner: &Address,
    dispatch: &NoticeId,
    scope: &HubAccount,
) -> Result<(), Failure> {
    let path = {
        let _disk = node.disk.lock();
        let incoming = incoming_path(node, owner, dispatch);
        let known: Option<Incoming> = read(&incoming).map_err(|error| Failure::new(error.to_string()))?;
        if known.as_ref().is_some_and(|known| known.scope != *scope) {
            return Err(Failure::refused("the run dispatch belongs to another account"));
        }
        write(&incoming.with_extension("stopped"), scope)
            .map_err(|error| Failure::new(format!("keeping the turn's cancellation: {error}")))?;
        known.map(|known| node.world.owner(&node.names(), owner).job_record(&known.job))
    };
    let Some(path) = path else {
        return Ok(());
    };
    let request = stop_request();
    loop {
        let record = match fastctx_hands::run::record::read(&path) {
            Ok(record) => record,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(Failure::new(error.to_string())),
        };
        if record.owner.resident() != Some(owner) || record.account.as_ref() != Some(scope) {
            return Err(Failure::refused("the run dispatch no longer names this owner's job"));
        }
        if record.end.is_some() {
            return Ok(());
        }
        crate::jobs::stop(node, &path, &request.by, &request.reason).await?;
        tokio::time::sleep(crate::on::LOOK_AGAIN).await;
    }
}

/// Request identities outlive their job so late duplicates cannot name a new job.
pub(crate) fn collect(node: &Node, age: std::time::Duration) -> io::Result<()> {
    for directory in node.world.book_directories("run-requests")? {
        for path in crate::retention::entries(&directory)? {
            if !matches!(path.extension().and_then(|ext| ext.to_str()), Some("json" | "stopped")) {
                continue;
            }
            if !path.metadata()?.modified()?.elapsed().is_ok_and(|elapsed| elapsed >= age) {
                continue;
            }
            if let Some(known) = read::<Incoming>(&path.with_extension("json"))? {
                let conversation = directory.parent().and_then(Path::parent).expect("conversation books");
                if conversation.join("jobs").join(format!("{}.json", known.job)).try_exists()? {
                    continue;
                }
            }
            std::fs::remove_file(path)?;
        }
    }
    for directory in node.world.book_directories("run-stops")? {
        for path in crate::retention::entries(&directory)? {
            if path.extension().is_some_and(|ext| ext == "answer")
                && !path.with_extension("json").try_exists()?
                && path.metadata()?.modified()?.elapsed().is_ok_and(|elapsed| elapsed >= age)
            {
                std::fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}
