//! A turn stop addresses one dispatch, never a conversation's next turn.
//! The receiver keeps a cancellation before acknowledging it, including when
//! its run has not arrived yet. Old peers can be stopped by a recovered job id.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_types::access::{Acting, HubAccount, Purpose, Written};
use fastctx_types::delivery_control::ControlLetter;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::run::{Ended, EndedHow, StopRequest, StopRequested};
use fastctx_types::{Address, JobId, NoticeId};
use serde::{Deserialize, Serialize};

use crate::Node;
use crate::obligations::Outcome;
use crate::records::items;

#[derive(Serialize, Deserialize)]
pub(crate) struct Pending {
    owner: Address,
    acting: Acting,
    destination: Written,
    dispatch: NoticeId,
    addressed: bool,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Incoming {
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
    items::get(path)
}

fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    items::keep(path, value)
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
    let (asking, toward) = (node.clone(), destination.clone());
    let addressed = fastctx_platform::task::blocking(move || crate::facts::run_stops(&asking, &toward)).await;
    let pending = Arc::new(Pending {
        owner: owner.clone(),
        acting: acting.clone(),
        destination: destination.clone(),
        dispatch: dispatch.clone(),
        addressed,
    });
    let mut again = crate::again::Again::default();
    loop {
        let (keeping, at, kept) = (node.clone(), path.clone(), pending.clone());
        let written = fastctx_platform::task::blocking(move || {
            let written = write(&at, &*kept);
            if let Err(error) = &written {
                let _ = crate::obligations::retry_record(&keeping, &at, &format!("keeping the turn's stop: {error}"));
            }
            written
        })
        .await;
        if written.is_ok() {
            break;
        }
        tokio::time::sleep(again.next()).await;
    }
    start(node, path, false);
    if !addressed {
        // An older receiver cannot address the dispatch. While the run still
        // waits there, the conversation-wide interruption it knows stops it at
        // once. It goes once and is never replayed, as a late one could stop a
        // later turn; the job named in the run's answer completes the stop.
        let (node, acting, destination) = (node.clone(), acting.clone(), destination.clone());
        let stop = RemoteCall::TurnInterrupted { resident: owner.resident.clone(), dispatch: None };
        fastctx_platform::task::spawn(async move {
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
    if items::exists(&path)? {
        write(&path.with_extension("answer"), &answered)?;
        node.control_letters.changed.notify_waiters();
    }
    Ok(())
}

pub(crate) fn resume(node: &Arc<Node>) {
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let mut again = crate::again::Again::default();
        loop {
            let recovering = node.clone();
            let result = fastctx_platform::task::spawn_blocking(move || {
                let result = (|| -> io::Result<()> {
                    for directory in recovering.world.book_directories("run-stops")? {
                        for path in items::entries(&directory, None)? {
                            if path.extension().is_some_and(|ext| ext == "json") {
                                start(&recovering, path, true);
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = &result {
                    let _ = crate::obligations::retry(
                        &recovering,
                        &recovering.world.path().join("run-stop-recovery"),
                        &error.to_string(),
                    );
                }
                result
            })
            .await;
            if matches!(result, Ok(Ok(()))) {
                return;
            }
            if let Err(error) = result {
                node.log.line(format_args!("restoring addressed stops: {error}"));
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
    fastctx_platform::task::spawn(async move {
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
                    let (keeping, at) = (node.clone(), path.clone());
                    let _ = fastctx_platform::task::blocking(move || {
                        crate::obligations::retry_record(&keeping, &at, &error.to_string())
                    })
                    .await;
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

/// What a turn stop's delivery comes to before any call it makes.
enum Attempt {
    /// It is settled: it was delivered or is no longer to be.
    Done(bool),
    /// It came to this, to be recorded.
    Came(Pending, Vec<u8>, Result<(), Failure>),
    /// An older receiver is to stop the job its run answered with.
    Call(Pending, Vec<u8>, JobId),
}

async fn attempt(node: &Arc<Node>, path: &Path, recovered: bool) -> io::Result<bool> {
    let (asking, at) = (node.clone(), path.to_path_buf());
    let (pending, bytes, result) =
        match fastctx_platform::task::blocking(move || prepare(&asking, &at, recovered)).await? {
            Attempt::Done(done) => return Ok(done),
            Attempt::Came(pending, bytes, result) => (pending, bytes, result),
            Attempt::Call(pending, bytes, job) => {
                let call = RemoteCall::StopJob { job, by: fastctx_types::residents::STOPPED_BY_USER.into() };
                let result = match node.call(&pending.acting, &pending.destination, Purpose::Hands, call).await {
                    Ok(Ok(RemoteReply::JobStopped { .. })) => Ok(()),
                    Ok(Ok(_)) => Err(Failure::new("the stop received an unrelated reply")),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(Failure::new(error.to_string())),
                };
                (pending, bytes, result)
            }
        };
    let (recording, at) = (node.clone(), path.to_path_buf());
    fastctx_platform::task::blocking(move || record(&recording, &at, &pending, &bytes, result)).await
}

/// Reads the pending stop at `path` and does what needs no call: whether it
/// can be delivered, the control letter that delivers it, or the answer an
/// older receiver's stop goes by.
fn prepare(node: &Arc<Node>, path: &Path, recovered: bool) -> io::Result<Attempt> {
    let bytes = match items::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Attempt::Done(true)),
        Err(error) => return Err(error),
    };
    let pending: Pending = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if let Some(reason) = crate::obligations::control_impossible(node, &pending.acting, &pending.destination)? {
        return Ok(Attempt::Came(pending, bytes, Err(Failure::refused(reason))));
    }
    if pending.addressed {
        let command = ControlLetter::StopRun { owner: pending.owner.clone(), dispatch: pending.dispatch.clone() };
        let Some(result) =
            crate::control_posts::send(node, path, &bytes, &pending.acting, &pending.destination, command)?
        else {
            return Ok(Attempt::Done(false));
        };
        return Ok(Attempt::Came(pending, bytes, result));
    }
    // A legacy resident-wide interruption cannot be replayed safely: a
    // later turn might already be waiting there. Its concrete job is safe.
    let answer = path.with_extension("answer");
    let answered: Answered = match items::read(&answer) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !recovered => return Ok(Attempt::Done(false)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Answered::Unknown,
        Err(error) => return Err(error),
    };
    match answered {
        Answered::Job(job) => Ok(Attempt::Call(pending, bytes, job)),
        Answered::NotRun => {
            crate::obligations::finish_record(node, path, &bytes, Outcome::Superseded, "the run never started there")?;
            Ok(Attempt::Done(true))
        }
        Answered::Unknown => Ok(Attempt::Came(
            pending,
            bytes,
            Err(Failure::refused(
                "this receiver cannot address a turn stop and its job identity was not recovered; upgrade it to deliver stops after a lost answer",
            )),
        )),
    }
}

/// Records what the delivery of the pending stop at `path` came to; says
/// whether it is settled.
fn record(
    node: &Arc<Node>,
    path: &Path,
    pending: &Pending,
    bytes: &[u8],
    result: Result<(), Failure>,
) -> io::Result<bool> {
    match result {
        Ok(()) => {
            crate::obligations::finish_record(
                node,
                path,
                bytes,
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
            crate::obligations::finish_record(node, path, bytes, Outcome::Impossible, &error.message)?;
        }
        Err(error) => {
            crate::obligations::retry_record(node, path, &error.message)?;
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
        items::create(&incoming, &serde_json::to_vec(&Incoming { job: record.id.clone(), scope: scope.clone() })?)?;
        crate::job_metadata::put_raw(&path.with_extension("run-request"), dispatch.as_str().as_bytes())?;
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
    let (keeping, whose, which, account) = (node.clone(), owner.clone(), dispatch.clone(), scope.clone());
    let path = fastctx_platform::task::blocking(move || {
        let _disk = keeping.disk.lock();
        let incoming = incoming_path(&keeping, &whose, &which);
        let known: Option<Incoming> = read(&incoming).map_err(|error| Failure::new(error.to_string()))?;
        if known.as_ref().is_some_and(|known| known.scope != account) {
            return Err(Failure::refused("the run dispatch belongs to another account"));
        }
        write(&incoming.with_extension("stopped"), &account)
            .map_err(|error| Failure::new(format!("keeping the turn's cancellation: {error}")))?;
        Ok(known.map(|known| keeping.world.owner(&keeping.names(), &whose).job_record(&known.job)))
    })
    .await?;
    let Some(path) = path else {
        return Ok(());
    };
    let request = stop_request();
    loop {
        let (looking, whose, which, account) = (path.clone(), owner.clone(), dispatch.clone(), scope.clone());
        if !fastctx_platform::task::blocking(move || running(&looking, &whose, &which, &account)).await? {
            return Ok(());
        }
        crate::jobs::stop(node, &path, &request.by, &request.reason).await?;
        tokio::time::sleep(crate::on::LOOK_AGAIN).await;
    }
}

/// Whether the job whose record is at `path` still runs for this dispatch.
fn running(path: &Path, owner: &Address, dispatch: &NoticeId, scope: &HubAccount) -> Result<bool, Failure> {
    let record = match fastctx_hands::run::record::read(path) {
        Ok(record) => record,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Failure::new(error.to_string())),
    };
    if record.owner.resident() != Some(owner) || record.account.as_ref() != Some(scope) {
        return Err(Failure::refused("the run dispatch no longer names this owner's job"));
    }
    if record.end.is_some() {
        return Ok(false);
    }
    // A job's short id is used again once its files are gone: the job there
    // now is this dispatch's only while it names the dispatch. One started by
    // another has nothing to do with this stop, whose job ended long since.
    match crate::job_metadata::read(&path.with_extension("run-request")) {
        Ok(named) if named == dispatch.as_str().as_bytes() => Ok(true),
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Failure::new(format!("reading which dispatch started the job: {error}"))),
    }
}

/// Request identities outlive their job so late duplicates cannot name a new job.
pub(crate) fn collect(node: &Node, age: std::time::Duration) -> io::Result<()> {
    for directory in node.world.book_directories("run-requests")? {
        for path in items::entries(&directory, None)? {
            if !matches!(path.extension().and_then(|ext| ext.to_str()), Some("json" | "stopped")) {
                continue;
            }
            if !items::modified(&path)?.elapsed().is_ok_and(|elapsed| elapsed >= age) {
                continue;
            }
            if let Some(known) = read::<Incoming>(&path.with_extension("json"))? {
                let conversation = directory.parent().and_then(Path::parent).expect("conversation books");
                if fastctx_platform::disk::exists(conversation.join("jobs").join(format!("{}.json", known.job)))? {
                    continue;
                }
            }
            items::delete(&path)?;
        }
    }
    for directory in node.world.book_directories("run-stops")? {
        for path in items::entries(&directory, None)? {
            if path.extension().is_some_and(|ext| ext == "answer")
                && !items::exists(&path.with_extension("json"))?
                && items::modified(&path)?.elapsed().is_ok_and(|elapsed| elapsed >= age)
            {
                items::delete(&path)?;
            }
        }
    }
    Ok(())
}
