//! `on` (`world-run.md`, 去别的机器): part of a script sent to another
//! machine, and the parts other machines send here.
//!
//! The sender collects everything first and sends it whole; the machine
//! that runs it starts an ordinary job only once the package is all there
//! and checks out. The job writes its output to that machine's disk, and
//! the sender follows it from byte offsets, so a connection that breaks is
//! taken up again where it broke and the job never depends on the sender
//! being there. The job's end goes to whoever follows it; when nobody does
//! at its end, it goes to the resident in a letter. Only the `on` that sent
//! it leaving on purpose stops it.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fastctx_net::data::Stream;
use fastctx_net::link::{CallFailure, Feed};
use fastctx_types::ipc::Failure;
use fastctx_types::on::{
    OnEnd, OnOrigin, OnPackage, OnPiece, OnRequest, OnSizes, OnWord, SILENT_FOR, STILL_EVERY, TAKEN,
};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::Action;
use fastctx_types::run::{EndedHow, JobOwner, JobRecord, OnJob, RunRequest};
use fastctx_types::{JobId, MachineName};
use sha2::{Digest as _, Sha256};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::Node;

/// The most bytes read from a log and handed over at once: one data
/// message (`fastctx_net::wire::MAX_DATA_MESSAGE`).
const CHUNK: usize = 1024 * 1024;

/// How long a follower waits before looking at a quiet log again. Output
/// growing says nothing to anyone, so it is looked for; a tenth of a second
/// is below what a reader notices, and a look at a file's end costs little.
pub(crate) const LOOK_AGAIN: Duration = Duration::from_millis(100);

const WAITING: &str = "waiting for the CLI receipt";

pub(crate) enum Piece {
    Value(Result<OnPiece, Failure>),
    End { end: OnEnd, taken: oneshot::Sender<()> },
}

#[derive(Clone)]
struct Pieces(mpsc::UnboundedSender<Piece>);

impl Pieces {
    fn send(&self, piece: Result<OnPiece, Failure>) -> Result<(), String> {
        self.0.send(Piece::Value(piece)).map_err(|_| "the on that sent it left".to_string())
    }

    fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    async fn closed(&self) {
        self.0.closed().await;
    }

    async fn ended(&self, end: OnEnd) -> Result<bool, String> {
        let (taken, receipt) = oneshot::channel();
        self.0.send(Piece::End { end, taken }).map_err(|_| "the on that sent it left".to_string())?;
        Ok(receipt.await.is_ok())
    }
}

// The sending side.

/// Sends `request` for the resident in `origin`. The job's pieces come on
/// the returned channel until its end or a failure; dropping the channel
/// before the end stops the job.
pub async fn send(
    node: &Arc<Node>,
    mut origin: OnOrigin,
    request: OnRequest,
) -> Result<mpsc::UnboundedReceiver<Piece>, Failure> {
    origin.request = Some(crate::speech::new_id("on"));
    let (pieces, receiver) = mpsc::unbounded_channel();
    let pieces = Pieces(pieces);
    let node = node.clone();
    tokio::spawn(async move {
        let waiting = Arc::new(Mutex::new(crate::on_delivery::Following::start(&node, &origin, &request.machine)));
        let local = request.machine == node.machine();
        let sent = if local {
            here(&node, &origin, &request, &pieces, &waiting).await
        } else {
            there(&node, &origin, &request, &pieces, &waiting).await
        };
        if let Err(mut failure) = sent {
            // Publication can fail after making a prepared debt visible.
            // Once a refusal is handed out, it cannot later become an end.
            if failure.not_run
                && let Err(error) = crate::on_delivery::cancelled(&node, &origin, &request.machine)
            {
                failure = Failure::new(format!("keeping the startup refusal: {error}"));
            }
            if local && !failure.not_run && pieces.is_closed() {
                crate::on_stop::send(&node, &request.machine, &origin, None).await;
            }
            let _ = pieces.send(Err(failure));
        }
    });
    Ok(receiver)
}

/// What the sender knows of a job it sent, kept across the connections
/// that follow it.
#[derive(Default)]
struct Followed {
    job: Option<JobId>,
    refused: Option<Failure>,
    out: u64,
    err: u64,
    ending: Option<EndReceipt>,
}

type EndReceipt = watch::Receiver<Option<Result<bool, Failure>>>;

async fn end_received(receipt: &mut EndReceipt) -> Result<bool, Failure> {
    loop {
        if let Some(result) = receipt.borrow_and_update().clone() {
            return result;
        }
        receipt.changed().await.map_err(|_| Failure::new("the completion receipt task stopped"))?;
    }
}

fn offer_end(
    followed: &Arc<Mutex<Followed>>,
    pieces: &Pieces,
    waiting: &Arc<Mutex<crate::on_delivery::Following>>,
    end: OnEnd,
) -> EndReceipt {
    let mut state = followed.lock().expect("never poisoned");
    if let Some(receipt) = &state.ending {
        return receipt.clone();
    }
    let job = state.job.clone().expect("started before end");
    let (done, receipt) = watch::channel(None);
    state.ending = Some(receipt.clone());
    let (pieces, waiting) = (pieces.clone(), waiting.clone());
    // This handover outlives a network feed. A late control reply must not
    // cancel a completion already queued for the CLI, or cause a second one.
    tokio::spawn(async move {
        let complete = !matches!(&end, OnEnd::Exited { remaining, .. } if *remaining > 0);
        let result = match pieces.ended(end).await {
            Ok(true) if complete => waiting
                .lock()
                .expect("never poisoned")
                .taken(&job)
                .map(|()| true)
                .map_err(|error| Failure::new(format!("keeping the completion receipt: {error}"))),
            Ok(taken) => Ok(taken),
            Err(_) => Ok(false),
        };
        let _ = done.send(Some(result));
    });
    receipt
}

/// What the sender does with a job's id once the machine says it started,
/// before any of its output is handed on.
type OnStarted = Arc<dyn Fn(JobId) -> fastctx_net::link::BoxFuture<Result<(), String>> + Send + Sync>;

/// An `on` naming this machine: the same job, without the network.
async fn here(
    node: &Arc<Node>,
    origin: &OnOrigin,
    request: &OnRequest,
    pieces: &Pieces,
    waiting: &Arc<Mutex<crate::on_delivery::Following>>,
) -> Result<(), Failure> {
    let staged = Staged::local(&request.package).await.map_err(|failure| Failure::not_run(failure.message))?;
    let (job, record_path, follower) = start(node, origin, staged, request.package.writes_off).await?;
    waiting.lock().expect("never poisoned").started(&job);
    noted(node, origin, request, &job).await?;
    let _ = pieces.send(Ok(OnPiece::Started { machine: node.machine(), job: job.clone() }));
    let mut sink = Sink::Local(pieces);
    let handed = hand_over(node, &job, &record_path, 0, 0, &mut sink).await;
    if matches!(handed, Ok(Handed { taken: true, complete: true })) {
        waiting
            .lock()
            .expect("never poisoned")
            .taken(&job)
            .map_err(|error| Failure::new(format!("keeping the completion receipt: {error}")))?;
    }
    // A local follower has no network to lose: its departure is an
    // intentional stop, just as it is for the sending side of a remote on.
    let stopped = if pieces.is_closed() && !matches!(handed, Ok(Handed { taken: true, .. })) {
        crate::on_stop::send(node, &request.machine, origin, Some(&job)).await;
        waiting
            .lock()
            .expect("never poisoned")
            .taken(&job)
            .map_err(|error| Failure::new(format!("keeping the intentional stop: {error}")))
    } else {
        Ok(())
    };
    let finished = follower.finish(handed).await;
    stopped.and(finished)
}

/// An `on` naming another machine.
async fn there(
    node: &Arc<Node>,
    origin: &OnOrigin,
    request: &OnRequest,
    pieces: &Pieces,
    waiting: &Arc<Mutex<crate::on_delivery::Following>>,
) -> Result<(), Failure> {
    let link = node
        .link()
        .ok_or_else(|| Failure::not_run(format!("{} is not connected to any other machine", node.machine())))?;
    let machine = request.machine.clone();
    let sizes = sizes(&request.package).await.map_err(|failure| Failure::not_run(failure.message))?;
    let _reaching =
        (origin.resident.machine == node.machine()).then(|| node.reaching.hold(&origin.resident.resident, &machine));
    requested(node, origin, request).await?;
    crate::on_delivery::keep_request(node, origin, &machine)
        .map_err(|error| Failure::not_run(format!("keeping the request before sending it: {error}")))?;
    let followed = Arc::new(Mutex::new(Followed::default()));
    // Noted when the machine says the job started, before its output is
    // handed on: the script's answer may be read as soon as the job is through.
    let started: OnStarted = {
        let (node, origin, request, waiting) = (node.clone(), origin.clone(), request.clone(), waiting.clone());
        Arc::new(move |job| {
            let (node, origin, request) = (node.clone(), origin.clone(), request.clone());
            waiting.lock().expect("never poisoned").started(&job);
            Box::pin(async move { noted(&node, &origin, &request, &job).await.map_err(|failure| failure.message) })
        })
    };
    let call = RemoteCall::On { origin: origin.clone(), sizes, writes_off: request.package.writes_off };
    let sending =
        feed(&machine, Some(request.package.clone()), followed.clone(), pieces.clone(), Some(started), waiting.clone());
    let first = link.call_with_stream(&machine, call, sending).await;
    let (started, refused) = {
        let followed = followed.lock().expect("never poisoned");
        (followed.job.clone(), followed.refused.clone())
    };
    if let Some(failure) = refused {
        return Err(failure);
    }
    let Some(job) = started else {
        let not_sent = matches!(
            &first,
            Err(CallFailure::NotConnected { .. }
                | CallFailure::NotMember { .. }
                | CallFailure::Offline { .. }
                | CallFailure::Unreachable { .. })
        ) || matches!(&first, Ok(Err(failure)) if failure.not_run);
        if pieces.is_closed() && !not_sent {
            crate::on_stop::send(node, &machine, origin, None).await;
            return Ok(());
        }
        return Err(match first {
            Ok(Err(failure)) => failure,
            Ok(Ok(_)) => {
                Failure::new(format!("{machine} answered without a startup confirmation; execution is unknown"))
            }
            // Nothing ran there: the call did not get through.
            Err(
                failure @ (CallFailure::NotConnected { .. }
                | CallFailure::NotMember { .. }
                | CallFailure::Offline { .. }
                | CallFailure::Unreachable { .. }),
            ) => Failure::not_run(failure.to_string()),
            Err(failure) => Failure::new(format!(
                "{failure}. If the job started there, its end will be told to you; `fastctx w` on {machine} shows it"
            )),
        });
    };
    let followed_through = follow_there(node, origin, &machine, &job, &followed, pieces, waiting).await;
    if matches!(followed_through, Ok(true)) {
        waiting
            .lock()
            .expect("never poisoned")
            .taken(&job)
            .map_err(|error| Failure::new(format!("keeping the intentional stop: {error}")))?;
    }
    followed_through.map(|_| ())
}

/// Follows a job that started on `machine` until its end is taken, it is
/// stopped, or the World says it cannot be followed. True when the `on`
/// that sent it left, and so stopped it.
async fn follow_there(
    node: &Arc<Node>,
    origin: &OnOrigin,
    machine: &MachineName,
    job: &JobId,
    followed: &Arc<Mutex<Followed>>,
    pieces: &Pieces,
    waiting: &Arc<Mutex<crate::on_delivery::Following>>,
) -> Result<bool, Failure> {
    let link = node.link().ok_or_else(|| Failure::new("the node left the network while following the job"))?;
    let mut delay = Backoff::default();
    loop {
        if let Some(failure) = followed.lock().expect("never poisoned").refused.clone() {
            return Err(failure);
        }
        let ending = followed.lock().expect("never poisoned").ending.clone();
        if let Some(mut receipt) = ending
            && end_received(&mut receipt).await?
        {
            return Ok(false);
        }
        if pieces.is_closed() {
            // The `on` that sent it left: it was stopped on purpose.
            crate::on_stop::send(node, machine, origin, Some(job)).await;
            return Ok(true);
        }
        tokio::select! {
            () = tokio::time::sleep(delay.next()) => {}
            () = pieces.closed() => continue,
        }
        let (out_from, err_from) = {
            let followed = followed.lock().expect("never poisoned");
            (followed.out, followed.err)
        };
        let call = RemoteCall::Follow { origin: origin.clone(), job: job.clone(), out_from, err_from };
        let following = feed(machine, None, followed.clone(), pieces.clone(), None, waiting.clone());
        let result = link.call_with_stream(machine, call, following).await;
        // A control reply can fail after the data stream already offered
        // the end. Only that handover's CLI receipt decides its outcome.
        if followed.lock().expect("never poisoned").ending.is_some() {
            continue;
        }
        match result {
            Ok(Err(failure)) => return Err(Failure::new(format!("following {job} on {machine}: {failure}"))),
            Err(CallFailure::NotMember { name, .. }) => {
                return Err(Failure::new(format!(
                    "{name} is no longer one of the user's machines; {job} was left there"
                )));
            }
            Ok(Ok(_)) | Err(_) => {}
        }
    }
}

/// The source end of an `On` or `Follow` connection: the package first, if
/// any, then what the job writes, until its end.
fn feed(
    machine: &MachineName,
    package: Option<OnPackage>,
    followed: Arc<Mutex<Followed>>,
    pieces: Pieces,
    started: Option<OnStarted>,
    waiting: Arc<Mutex<crate::on_delivery::Following>>,
) -> Feed {
    let machine = machine.clone();
    Box::new(move |mut stream: Stream| {
        Box::pin(async move {
            if let Some(package) = package {
                send_package(&mut stream, &package).await?;
            }
            loop {
                let word = tokio::select! {
                    word = tokio::time::timeout(SILENT_FOR, stream.hear::<OnWord>()) => match word {
                        Ok(word) => word?,
                        Err(_) => return Err(format!("{machine} said nothing for {}s", SILENT_FOR.as_secs())),
                    },
                    () = pieces.closed() => return Err("the on that sent it left".to_string()),
                };
                match word {
                    OnWord::Refused { failure } => {
                        followed.lock().expect("never poisoned").refused = Some(failure);
                        return stream.say(&TAKEN).await;
                    }
                    OnWord::Started { job } => {
                        followed.lock().expect("never poisoned").job = Some(job.clone());
                        if let Some(started) = &started
                            && let Err(error) = started(job.clone()).await
                        {
                            followed.lock().expect("never poisoned").refused = Some(Failure::new(error.clone()));
                            return Err(error);
                        }
                        let _ = pieces.send(Ok(OnPiece::Started { machine: machine.clone(), job }));
                    }
                    OnWord::Out { bytes } | OnWord::Err { bytes } => {
                        let error = matches!(word, OnWord::Err { .. });
                        let mut taken = Vec::new();
                        stream
                            .take(bytes, |chunk| {
                                taken.extend_from_slice(chunk);
                                Ok(())
                            })
                            .await?;
                        {
                            let mut followed = followed.lock().expect("never poisoned");
                            if error { followed.err += bytes } else { followed.out += bytes }
                        }
                        let base64 = STANDARD.encode(&taken);
                        let piece = if error { OnPiece::Err { base64 } } else { OnPiece::Out { base64 } };
                        if pieces.send(Ok(piece)).is_err() {
                            return Err("the on that sent it left".to_string());
                        }
                    }
                    OnWord::Still => {}
                    OnWord::Ended { end, .. } => {
                        let mut receipt = offer_end(&followed, &pieces, &waiting, end);
                        let taken = loop {
                            tokio::select! {
                                result = end_received(&mut receipt) => break result.map_err(|failure| failure.message)?,
                                () = tokio::time::sleep(STILL_EVERY) => stream.say(&WAITING).await?,
                            }
                        };
                        if !taken {
                            return Err("the on that sent it left".to_string());
                        }
                        return stream.say(&TAKEN).await;
                    }
                }
            }
        })
    })
}

/// Sends the package's parts in the order `OnSizes` lists them.
async fn send_package(stream: &mut Stream, package: &OnPackage) -> Result<(), String> {
    for path in [Some(&package.state), Some(&package.script), package.input.as_ref()].into_iter().flatten() {
        let mut file = tokio::fs::File::open(path).await.map_err(|error| format!("reading {path}: {error}"))?;
        let mut buffer = vec![0u8; CHUNK];
        loop {
            let read = tokio::io::AsyncReadExt::read(&mut file, &mut buffer)
                .await
                .map_err(|error| format!("reading {path}: {error}"))?;
            if read == 0 {
                break;
            }
            stream.send(&buffer[..read]).await?;
        }
    }
    Ok(())
}

/// The package's sizes and digest, read once before anything is sent.
async fn sizes(package: &OnPackage) -> Result<OnSizes, Failure> {
    let package = package.clone();
    tokio::task::spawn_blocking(move || -> std::io::Result<OnSizes> {
        let mut digest = Sha256::new();
        let mut size = |path: Option<&String>| -> std::io::Result<Option<u64>> {
            let Some(path) = path else { return Ok(None) };
            let mut file = std::fs::File::open(path)?;
            let mut buffer = vec![0u8; CHUNK];
            let mut total = 0u64;
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    return Ok(Some(total));
                }
                digest.update(&buffer[..read]);
                total += read as u64;
            }
        };
        let state = size(Some(&package.state))?.expect("given");
        let script = size(Some(&package.script))?.expect("given");
        let input = size(package.input.as_ref())?.unwrap_or(0);
        Ok(OnSizes { state, script, input, sha256: hex(&digest.finalize()) })
    })
    .await
    .map_err(|error| Failure::new(format!("reading the package: {error}")))?
    .map_err(|error| Failure::new(format!("reading the package: {error}")))
}

/// Records the `on` where it was written: in the resident's actions, when
/// it lives here, so the machine it went to may write back about it; and
/// beside the job whose script it is part of, for that job's answer.
async fn requested(node: &Arc<Node>, origin: &OnOrigin, request: &OnRequest) -> Result<(), Failure> {
    if origin.resident.machine != node.machine() {
        return Ok(());
    }
    let action = Action {
        at: jiff::Timestamp::now(),
        machine: request.machine.clone(),
        hand: "on".to_string(),
        what: "sending a script".to_string(),
        result: String::new(),
    };
    let (world, origin) = (node.world.clone(), origin.clone());
    tokio::task::spawn_blocking(move || {
        let directory = world.resident(origin.town.as_ref(), &origin.resident.resident);
        let (_transaction, directory) = crate::resident_storage::current(&directory)?;
        let mut bytes = match std::fs::read(directory.actions()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        // A prior append may have failed mid-line. Preserve that fragment,
        // but do not let it swallow the durable action for this request.
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend(serde_json::to_vec(&action).map_err(std::io::Error::other)?);
        bytes.push(b'\n');
        fastctx_platform::publish(&directory.actions(), &bytes, fastctx_platform::Permissions::Private)
    })
    .await
    .map_err(|error| Failure::not_run(format!("recording the request before sending it: {error}")))?
    .map_err(|error| Failure::not_run(format!("recording the request before sending it: {error}")))
}

async fn noted(node: &Arc<Node>, origin: &OnOrigin, request: &OnRequest, job: &JobId) -> Result<(), Failure> {
    let here = node.machine();
    let action = Action {
        at: jiff::Timestamp::now(),
        machine: request.machine.clone(),
        hand: "on".to_string(),
        what: job.to_string(),
        result: String::new(),
    };
    let (world, origin, request, job) = (node.world.clone(), origin.clone(), request.clone(), job.clone());
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let owner = JobOwner::Resident(origin.resident.clone());
        if origin.resident.machine == here {
            crate::residents::record_action(&world, &origin.resident.resident, origin.town.as_ref(), &action)?;
        }
        if let Some(parent) = &request.job {
            let directory = world.job_owner(&here, origin.town.as_ref(), &owner);
            let line = format!("{} {job}\n", request.machine);
            append(&directory.jobs().join(format!("{parent}.on")), line.as_bytes())?;
        }
        Ok(())
    })
    .await
    .map_err(|error| Failure::new(format!("recording the started job: {error}")))?
    .map_err(|error| Failure::new(format!("recording the started job: {error}")))
}

fn append(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(bytes)
}

/// The waits between attempts to reach a machine again, as the link waits
/// to reconnect (`world-network.md`): at once, then up to twice as long
/// each time, at most a minute, each a random part of that.
#[derive(Default)]
pub(crate) struct Backoff {
    ceiling: Option<Duration>,
}

impl Backoff {
    pub(crate) fn next(&mut self) -> Duration {
        let ceiling = match self.ceiling {
            None => {
                self.ceiling = Some(Duration::from_secs(1));
                return Duration::ZERO;
            }
            Some(ceiling) => ceiling,
        };
        self.ceiling = Some((ceiling * 2).min(Duration::from_secs(60)));
        let mut byte = [0u8; 1];
        let _ = getrandom::fill(&mut byte);
        ceiling / 2 + ceiling.mul_f64(f64::from(byte[0]) / 512.0)
    }
}

// The receiving side.

/// Runs the package an `On` call from `from` brings, and hands over the
/// job's output on `stream` until its end.
pub async fn serve_on(
    node: &Arc<Node>,
    origin: OnOrigin,
    sizes: OnSizes,
    writes_off: bool,
    mut stream: Stream,
) -> Result<RemoteReply, Failure> {
    let starting = async {
        let staged = Staged::receive(node, &origin, &sizes, &mut stream)
            .await
            .map_err(|failure| Failure::not_run(failure.message))?;
        start(node, &origin, staged, writes_off).await
    }
    .await;
    let (job, record_path, follower) = match starting {
        Ok(started) => started,
        Err(failure) if !failure.not_run => return Err(failure),
        Err(failure) => {
            // Closing the data stream first races the control reply and
            // turns a definite refusal into an unknown execution outcome.
            // Hand over the refusal on the stream before answering the call.
            stream
                .say(&OnWord::Refused { failure: failure.clone() })
                .await
                .map_err(|error| Failure::new(format!("saying the job was not started: {error}")))?;
            match tokio::time::timeout(SILENT_FOR, stream.hear::<String>()).await {
                Ok(Ok(word)) if word == TAKEN => return Err(failure),
                Ok(Ok(_)) => return Err(Failure::new("the sender did not acknowledge the refusal")),
                Ok(Err(error)) => return Err(Failure::new(error)),
                Err(_) => {
                    return Err(Failure::new("the sender did not acknowledge the refusal before its line went silent"));
                }
            }
        }
    };
    let handed = async {
        stream.say(&OnWord::Started { job: job.clone() }).await?;
        let mut sink = Sink::Stream(&mut stream);
        hand_over(node, &job, &record_path, 0, 0, &mut sink).await
    }
    .await;
    follower.finish(handed).await?;
    Ok(RemoteReply::OnDone)
}

/// Goes on handing over an `on` job's output from where a broken
/// connection left it.
pub async fn serve_follow(
    node: &Arc<Node>,
    origin: OnOrigin,
    job: JobId,
    out_from: u64,
    err_from: u64,
    mut stream: Stream,
) -> Result<RemoteReply, Failure> {
    let record_path = on_job(node, &origin, &job)?;
    let follower = Follower::new(node, &record_path);
    let mut sink = Sink::Stream(&mut stream);
    let handed = hand_over(node, &job, &record_path, out_from, err_from, &mut sink).await;
    follower.finish(handed).await?;
    Ok(RemoteReply::OnDone)
}

/// The record of the `on` job `job` of the resident in `origin`.
pub(crate) fn on_job(node: &Arc<Node>, origin: &OnOrigin, job: &JobId) -> Result<PathBuf, Failure> {
    let owner = JobOwner::Resident(origin.resident.clone());
    let path = node.world.job_owner(&node.machine(), origin.town.as_ref(), &owner).job_record(job);
    match fastctx_hands::run::record::read(&path) {
        Ok(record)
            if record
                .on
                .as_ref()
                .and_then(|on| on.request.as_ref())
                .is_some_and(|request| origin.request.as_ref() != Some(request)) =>
        {
            Err(Failure::new(format!("{job} belongs to a different on request")))
        }
        Ok(record) if record.on.is_some() => Ok(path),
        Ok(_) => Err(Failure::new(format!("{job} on {} was not sent by an on", node.machine()))),
        Err(error) => {
            Err(Failure::new(format!("no on job {job} of {} on {}: {error}", origin.resident, node.machine())))
        }
    }
}

/// A package on this machine, ready to become a job.
struct Staged {
    state: PathBuf,
    script: String,
    input: Option<PathBuf>,
}

impl Staged {
    /// The package `fastctx on` wrote here.
    async fn local(package: &OnPackage) -> Result<Self, Failure> {
        let script = tokio::fs::read_to_string(&package.script)
            .await
            .map_err(|error| Failure::new(format!("reading the script {}: {error}", package.script)))?;
        Ok(Self { state: PathBuf::from(&package.state), script, input: package.input.as_ref().map(PathBuf::from) })
    }

    /// The package an `On` call brings, written to the resident's directory
    /// here and checked against its digest before anything runs.
    async fn receive(
        node: &Arc<Node>,
        origin: &OnOrigin,
        sizes: &OnSizes,
        stream: &mut Stream,
    ) -> Result<Self, Failure> {
        let owner = JobOwner::Resident(origin.resident.clone());
        let directory = node.world.job_owner(&node.machine(), origin.town.as_ref(), &owner).out();
        let name = random_name().map_err(|error| Failure::new(format!("naming the package: {error}")))?;
        let created = directory.clone();
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&created))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("making {}: {error}", directory.display())))?;
        let mut digest = Sha256::new();
        let state = directory.join(format!("on-{name}.state"));
        take_to(stream, sizes.state, &state, &mut digest).await?;
        let mut bytes = Vec::new();
        stream
            .take(sizes.script, |chunk| {
                digest.update(chunk);
                bytes.extend_from_slice(chunk);
                Ok(())
            })
            .await
            .map_err(|error| Failure::new(format!("receiving the script: {error}")))?;
        let script = String::from_utf8(bytes).map_err(|_| Failure::new("the script is not UTF-8"))?;
        let input = if sizes.input > 0 {
            let path = directory.join(format!("on-{name}.in"));
            take_to(stream, sizes.input, &path, &mut digest).await?;
            Some(path)
        } else {
            None
        };
        if hex(&digest.clone().finalize()) != sizes.sha256 {
            return Err(Failure::new("the package arrived different from what was sent; nothing ran"));
        }
        Ok(Self { state, script, input })
    }
}

async fn take_to(stream: &mut Stream, size: u64, path: &Path, digest: &mut Sha256) -> Result<(), Failure> {
    use std::io::Write as _;
    let mut file =
        std::fs::File::create(path).map_err(|error| Failure::new(format!("writing {}: {error}", path.display())))?;
    let mut written = Ok(());
    stream
        .take(size, |chunk| {
            digest.update(chunk);
            if written.is_ok() {
                written = file.write_all(chunk);
            }
            Ok(())
        })
        .await
        .map_err(|error| Failure::new(format!("receiving the package: {error}")))?;
    written.map_err(|error| Failure::new(format!("writing {}: {error}", path.display())))
}

/// Starts the job a package holds, and marks its end as owed to the
/// resident: the follower takes that over until it leaves.
async fn start(
    node: &Arc<Node>,
    origin: &OnOrigin,
    staged: Staged,
    writes_off: bool,
) -> Result<(JobId, PathBuf, Follower), Failure> {
    let script = staged.script;
    let on = OnJob { state: staged.state.to_string_lossy().into_owned(), request: origin.request.clone() };
    let request = RunRequest { script, cwd: None, writes_off, wait: None };
    let owner = JobOwner::Resident(origin.resident.clone());
    let registration = Arc::new(Mutex::new(None));
    let launched = node
        .jobs
        .launch(node, owner, origin.town.clone(), request, Some((on, staged.input)), Some(registration.clone()))
        .await?;
    let follower = registration.lock().expect("never poisoned").take().expect("registered before spawn");
    Ok((launched.job, launched.record, follower))
}

pub(crate) type Registration = Arc<Mutex<Option<Follower>>>;

pub(crate) struct Follower {
    node: Arc<Node>,
    path: PathBuf,
}

impl Follower {
    pub(crate) fn new(node: &Arc<Node>, path: &Path) -> Self {
        *node.following.lock().expect("never poisoned").entry(path.to_path_buf()).or_default() += 1;
        Self { node: node.clone(), path: path.to_path_buf() }
    }

    async fn finish(self, handed: Result<Handed, String>) -> Result<(), Failure> {
        // A shell's background children still owe their eventual end,
        // even if they finished while the shell's receipt was in flight.
        if matches!(handed, Ok(Handed { taken: true, complete: true })) {
            fastctx_platform::publish(
                &self.path.with_extension("on-taken"),
                b"",
                fastctx_platform::Permissions::Private,
            )
            .map_err(|error| Failure::new(format!("keeping the completion receipt: {error}")))?;
        }
        Ok(())
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        {
            let mut following = self.node.following.lock().expect("never poisoned");
            let count = following.get_mut(&self.path).expect("registered follower");
            *count -= 1;
            if *count == 0 {
                following.remove(&self.path);
            }
        }
        crate::inbox::retry_job(&self.node, &self.path);
    }
}

/// Whether nobody follows `job` now, so its end goes in a letter.
pub fn unfollowed(node: &Node, record: &Path) -> bool {
    !node.following.lock().expect("never poisoned").contains_key(record)
}

/// Where a job's output is handed: the network, or this machine's own
/// `on`.
enum Sink<'a> {
    Stream(&'a mut Stream),
    Local(&'a Pieces),
}

struct Handed {
    taken: bool,
    complete: bool,
}

impl Sink<'_> {
    async fn bytes(&mut self, error: bool, bytes: &[u8]) -> Result<(), String> {
        match self {
            Sink::Stream(stream) => {
                let length = bytes.len() as u64;
                stream.say(&if error { OnWord::Err { bytes: length } } else { OnWord::Out { bytes: length } }).await?;
                stream.send(bytes).await
            }
            Sink::Local(pieces) => {
                let base64 = STANDARD.encode(bytes);
                let piece = if error { OnPiece::Err { base64 } } else { OnPiece::Out { base64 } };
                pieces.send(Ok(piece)).map_err(|_| "the on that sent it left".to_string())
            }
        }
    }

    async fn still(&mut self) -> Result<(), String> {
        match self {
            Sink::Stream(stream) => stream.say(&OnWord::Still).await,
            Sink::Local(_) => Ok(()),
        }
    }

    /// Hands over the end; true when the other side says it has it.
    async fn ended(&mut self, end: OnEnd, lettered: bool) -> Result<bool, String> {
        match self {
            Sink::Stream(stream) => {
                stream.say(&OnWord::Ended { end, lettered }).await?;
                loop {
                    match tokio::time::timeout(SILENT_FOR, stream.hear::<String>()).await {
                        Ok(Ok(word)) if word == WAITING => continue,
                        Ok(Ok(word)) => return Ok(word == TAKEN),
                        Ok(Err(error)) => return Err(error),
                        Err(_) => return Ok(false),
                    }
                }
            }
            Sink::Local(pieces) => pieces.ended(end).await,
        }
    }

    async fn closed(&mut self) {
        match self {
            Sink::Stream(stream) => stream.closed().await,
            Sink::Local(pieces) => pieces.closed().await,
        }
    }
}

/// Hands over the output of the job at `record_path` from the given byte
/// offsets until the job is through, then its end. True when the end was
/// taken; an error when the connection broke first.
async fn hand_over(
    node: &Arc<Node>,
    job: &JobId,
    record_path: &Path,
    mut out: u64,
    mut err: u64,
    sink: &mut Sink<'_>,
) -> Result<Handed, String> {
    let mut changes = node.jobs.subscribe();
    let mut quiet_since = Instant::now();
    loop {
        // The record first: whatever it says has ended had written its
        // last byte before, so reading the logs after it misses nothing.
        let record = crate::jobs::read(record_path).await.map_err(|failure| failure.message)?;
        let errors = record.stderr.clone();
        let boundary = record.output_drained.filter(|_| record.end.is_none() && through(&record).is_some());
        let mut sent = false;
        if let Some(bytes) = read_from(&record.output, out, boundary.map(|b| b.stdout)).await? {
            out += bytes.len() as u64;
            sink.bytes(false, &bytes).await?;
            sent = true;
        }
        if let Some(path) = &errors
            && let Some(bytes) = read_from(path, err, boundary.map(|b| b.stderr)).await?
        {
            err += bytes.len() as u64;
            sink.bytes(true, &bytes).await?;
            sent = true;
        }
        let drained = boundary.is_some_and(|b| out >= b.stdout && err >= b.stderr);
        if sent && !drained {
            quiet_since = Instant::now();
            continue;
        }
        if let Some(end) = through(&record) {
            // A letter goes only for an end nobody followed, and takes the
            // marker that it is owed: without one, the letter went.
            let owed = node.world.job_owner(&node.machine(), record.town.as_ref(), &record.owner).job_owed(job);
            let lettered = record.end.is_some() && matches!(tokio::fs::try_exists(&owed).await, Ok(false));
            let complete = record.end.is_some();
            return sink.ended(end, lettered).await.map(|taken| Handed { taken, complete });
        }
        if quiet_since.elapsed() >= STILL_EVERY {
            sink.still().await?;
            quiet_since = Instant::now();
        }
        tokio::select! {
            change = changes.recv() => {
                if let Ok(changed) = change && changed != *job {
                    continue;
                }
            }
            () = tokio::time::sleep(LOOK_AGAIN) => {}
            () = sink.closed() => return Err("the follower left".to_string()),
        }
    }
}

/// How the job stands for `on`, once its shell has exited and its output
/// has drained, or it ended otherwise: an `on` ends as a subshell does.
fn through(record: &JobRecord) -> Option<OnEnd> {
    let status = record.exit.map(|exit| exit.status);
    // Output the log could not take was never handed over: the `on` did not
    // carry the job through, whatever its code.
    if let Some(lost) = record.output_lost.as_ref().or(record.stderr_lost.as_ref())
        && (record.end.is_some()
            || (record.output_drained.is_some() && record.remaining.is_some_and(|remaining| remaining > 0)))
    {
        let error = format!("its output there stopped being kept after {} bytes: {}", lost.kept_bytes, lost.error);
        return Some(OnEnd::Otherwise { how: EndedHow::Failed { error }, status });
    }
    if let Some(end) = &record.end {
        return Some(match (&end.how, status) {
            (EndedHow::Finished, Some(status)) => OnEnd::Exited { status, remaining: 0 },
            (how, status) => OnEnd::Otherwise { how: how.clone(), status },
        });
    }
    match status {
        Some(status)
            if (record.output_drained.is_some() || record.output_ended_at.is_some())
                && record.remaining.is_some_and(|remaining| remaining > 0) =>
        {
            Some(OnEnd::Exited { status, remaining: record.remaining.expect("checked") })
        }
        _ => None,
    }
}

/// Up to `CHUNK` bytes of the file at `path` from `offset`, when there are
/// any.
async fn read_from(path: &str, offset: u64, through: Option<u64>) -> Result<Option<Vec<u8>>, String> {
    let path = PathBuf::from(path);
    tokio::task::spawn_blocking(move || -> std::io::Result<Option<Vec<u8>>> {
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let end = through.unwrap_or(file.metadata()?.len());
        if end <= offset {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut buffer = vec![0u8; (end - offset).min(CHUNK as u64) as usize];
        let read = file.read(&mut buffer)?;
        buffer.truncate(read);
        Ok((read > 0).then_some(buffer))
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("reading the output: {error}"))
}

fn random_name() -> std::io::Result<String> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
