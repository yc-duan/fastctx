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
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::Node;

/// The most bytes read from a log and handed over at once: one data
/// message (`fastctx_net::wire::MAX_DATA_MESSAGE`).
const CHUNK: usize = 1024 * 1024;

/// How long a follower waits before looking at a quiet log again. Output
/// growing says nothing to anyone, so it is looked for; a tenth of a second
/// is below what a reader notices, and a look at a file's end costs little.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

type Pieces = mpsc::UnboundedSender<Result<OnPiece, Failure>>;

// The sending side.

/// Sends `request` for the resident in `origin`. The job's pieces come on
/// the returned channel until its end or a failure; dropping the channel
/// before the end stops the job.
pub async fn send(
    node: &Arc<Node>,
    origin: OnOrigin,
    request: OnRequest,
) -> Result<mpsc::UnboundedReceiver<Result<OnPiece, Failure>>, Failure> {
    let (pieces, receiver) = mpsc::unbounded_channel();
    let node = node.clone();
    tokio::spawn(async move {
        let sent = if request.machine == node.machine() {
            here(&node, &origin, &request, &pieces).await
        } else {
            there(&node, &origin, &request, &pieces).await
        };
        if let Err(failure) = sent {
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
    out: u64,
    err: u64,
    ended: bool,
}

/// What the sender does with a job's id once the machine says it started,
/// before any of its output is handed on.
type OnStarted = Arc<dyn Fn(JobId) -> fastctx_net::link::BoxFuture<()> + Send + Sync>;

/// An `on` naming this machine: the same job, without the network.
async fn here(node: &Arc<Node>, origin: &OnOrigin, request: &OnRequest, pieces: &Pieces) -> Result<(), Failure> {
    let staged = Staged::local(&request.package).await?;
    let (job, record_path) = start(node, origin, staged, request.package.writes_off).await?;
    noted(node, origin, request, &job).await;
    let _ = pieces.send(Ok(OnPiece::Started { machine: node.machine(), job: job.clone() }));
    let mut sink = Sink::Local(pieces);
    let handed = hand_over(node, &job, &record_path, 0, 0, &mut sink).await;
    finish(node, &job, &record_path, handed).await
}

/// An `on` naming another machine.
async fn there(node: &Arc<Node>, origin: &OnOrigin, request: &OnRequest, pieces: &Pieces) -> Result<(), Failure> {
    let link =
        node.link().ok_or_else(|| Failure::new(format!("{} is in no World with other machines", node.machine())))?;
    let machine = request.machine.clone();
    let sizes = sizes(&request.package).await?;
    let followed = Arc::new(Mutex::new(Followed::default()));
    // Noted when the machine says the job started, before its output is
    // handed on: the script's answer may be read as soon as the job is through.
    let started: OnStarted = {
        let (node, origin, request) = (node.clone(), origin.clone(), request.clone());
        Arc::new(move |job| {
            let (node, origin, request) = (node.clone(), origin.clone(), request.clone());
            Box::pin(async move { noted(&node, &origin, &request, &job).await })
        })
    };
    let call = RemoteCall::On { origin: origin.clone(), sizes, writes_off: request.package.writes_off };
    let sending = feed(&machine, Some(request.package.clone()), followed.clone(), pieces.clone(), Some(started));
    let first = link.call_with_stream(&machine, call, sending).await;
    let started = followed.lock().expect("never poisoned").job.clone();
    let Some(job) = started else {
        return Err(match first {
            Ok(Err(failure)) => failure,
            Ok(Ok(_)) => Failure::new(format!("{machine} answered without starting the job")),
            // Nothing ran there: the call did not get through.
            Err(
                failure @ (CallFailure::NotConnected { .. }
                | CallFailure::NotMember { .. }
                | CallFailure::Offline { .. }
                | CallFailure::Unreachable { .. }),
            ) => Failure::new(failure.to_string()),
            Err(failure) => Failure::new(format!(
                "{failure}. If the job started there, its end will be told to you; `fastctx w` on {machine} shows it"
            )),
        });
    };
    let mut delay = Backoff::default();
    loop {
        if followed.lock().expect("never poisoned").ended {
            return Ok(());
        }
        if pieces.is_closed() {
            // The `on` that sent it left: it was stopped on purpose.
            stop_there(node, &machine, origin, &job).await;
            return Ok(());
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
        let following = feed(&machine, None, followed.clone(), pieces.clone(), None);
        match link.call_with_stream(&machine, call, following).await {
            Ok(Err(failure)) => return Err(Failure::new(format!("following {job} on {machine}: {failure}"))),
            Err(CallFailure::NotMember { name, .. }) => {
                return Err(Failure::new(format!("{name} is no longer a machine of the World; {job} was left there")));
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
                    OnWord::Started { job } => {
                        followed.lock().expect("never poisoned").job = Some(job.clone());
                        if let Some(started) = &started {
                            started(job.clone()).await;
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
                    OnWord::Ended { end } => {
                        if pieces.send(Ok(OnPiece::Ended { end })).is_err() {
                            return Err("the on that sent it left".to_string());
                        }
                        followed.lock().expect("never poisoned").ended = true;
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
async fn noted(node: &Arc<Node>, origin: &OnOrigin, request: &OnRequest, job: &JobId) {
    let here = node.machine();
    let action = Action {
        at: jiff::Timestamp::now(),
        machine: request.machine.clone(),
        hand: "on".to_string(),
        what: job.to_string(),
        result: String::new(),
    };
    let (world, origin, request, job) = (node.world.clone(), origin.clone(), request.clone(), job.clone());
    let _ = tokio::task::spawn_blocking(move || {
        let owner = JobOwner::Resident(origin.resident.clone());
        if origin.resident.machine == here {
            let _ = crate::residents::record_action(&world, &origin.resident.resident, origin.town.as_ref(), &action);
        }
        if let Some(parent) = &request.job {
            let directory = world.job_owner(&here, origin.town.as_ref(), &owner);
            let line = format!("{} {job}\n", request.machine);
            let _ = append(&directory.jobs().join(format!("{parent}.on")), line.as_bytes());
        }
    })
    .await;
}

fn append(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(bytes)
}

/// Asks `machine` to stop `job`, until it has or the job is gone: the stop
/// was meant, so it is not dropped for a broken connection.
async fn stop_there(node: &Arc<Node>, machine: &MachineName, origin: &OnOrigin, job: &JobId) {
    let Some(link) = node.link() else { return };
    let mut delay = Backoff::default();
    loop {
        let call = RemoteCall::StopOn { origin: origin.clone(), job: job.clone() };
        match link.call(machine, call).await {
            Ok(_) | Err(CallFailure::NotMember { .. }) => return,
            Err(_) => tokio::time::sleep(delay.next()).await,
        }
    }
}

/// The waits between attempts to reach a machine again, as the link waits
/// to reconnect (`world-network.md`): at once, then up to twice as long
/// each time, at most a minute, each a random part of that.
#[derive(Default)]
struct Backoff {
    ceiling: Option<Duration>,
}

impl Backoff {
    fn next(&mut self) -> Duration {
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
    let staged = Staged::receive(node, &origin, &sizes, &mut stream).await?;
    let (job, record_path) = start(node, &origin, staged, writes_off).await?;
    stream
        .say(&OnWord::Started { job: job.clone() })
        .await
        .map_err(|error| Failure::new(format!("saying the job started: {error}")))?;
    let mut sink = Sink::Stream(&mut stream);
    let handed = hand_over(node, &job, &record_path, 0, 0, &mut sink).await;
    finish(node, &job, &record_path, handed).await?;
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
    follow(node, &job);
    let mut sink = Sink::Stream(&mut stream);
    let handed = hand_over(node, &job, &record_path, out_from, err_from, &mut sink).await;
    finish(node, &job, &record_path, handed).await?;
    Ok(RemoteReply::OnDone)
}

/// Stops an `on` job its sender stopped.
pub async fn serve_stop(node: &Arc<Node>, origin: OnOrigin, job: JobId) -> Result<RemoteReply, Failure> {
    let record_path = on_job(node, &origin, &job)?;
    let by = format!("{} on {}", origin.resident.resident, origin.resident.machine);
    crate::jobs::stop(node, &record_path, &by, "the on that sent it stopped").await?;
    Ok(RemoteReply::OnDone)
}

/// The record of the `on` job `job` of the resident in `origin`.
fn on_job(node: &Arc<Node>, origin: &OnOrigin, job: &JobId) -> Result<PathBuf, Failure> {
    let owner = JobOwner::Resident(origin.resident.clone());
    let path = node.world.job_owner(&node.machine(), origin.town.as_ref(), &owner).job_record(job);
    match fastctx_hands::run::record::read(&path) {
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
) -> Result<(JobId, PathBuf), Failure> {
    let script = staged.script;
    let on = OnJob { state: staged.state.to_string_lossy().into_owned() };
    let request = RunRequest { script, cwd: None, writes_off };
    let owner = JobOwner::Resident(origin.resident.clone());
    let launched = node.jobs.launch(node, owner, origin.town.clone(), request, Some((on, staged.input))).await?;
    // Followed before its end is owed, so an end that comes first goes to
    // the follower rather than in a letter too.
    follow(node, &launched.job);
    let (world, here, job, path) = (node.world.clone(), node.machine(), launched.job.clone(), launched.record.clone());
    tokio::task::spawn_blocking(move || crate::inbox::owe_job(&world, &here, &job, &path))
        .await
        .map_err(|error| Failure::new(format!("noting that the job's end is owed: {error}")))?
        .map_err(|error| Failure::new(format!("noting that the job's end is owed: {error}")))?;
    Ok((launched.job, launched.record))
}

/// Marks `job` as followed by one more follower.
fn follow(node: &Arc<Node>, job: &JobId) {
    *node.following.lock().expect("never poisoned").entry(job.clone()).or_default() += 1;
}

/// After a follower is done with `job`: a handed-over end is no longer
/// owed; otherwise, with no follower left, the resident is told by letter
/// if the job has ended, or when it does.
async fn finish(
    node: &Arc<Node>,
    job: &JobId,
    record_path: &Path,
    handed: Result<bool, String>,
) -> Result<(), Failure> {
    {
        let mut following = node.following.lock().expect("never poisoned");
        if let Some(count) = following.get_mut(job) {
            *count -= 1;
            if *count == 0 {
                following.remove(job);
            }
        }
    }
    let record = crate::jobs::read(record_path).await?;
    match handed {
        Ok(true) => {
            let owed = node.world.job_owner(&node.machine(), record.town.as_ref(), &record.owner).job_owed(job);
            match tokio::fs::remove_file(&owed).await {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(Failure::new(format!("{}: {error}", owed.display())))
                }
                _ => Ok(()),
            }
        }
        Ok(false) | Err(_) => crate::inbox::tell_if_ended(node, &record)
            .await
            .map_err(|error| Failure::new(format!("telling the job's end: {error}"))),
    }
}

/// Whether nobody follows `job` now, so its end goes in a letter.
pub fn unfollowed(node: &Node, job: &JobId) -> bool {
    !node.following.lock().expect("never poisoned").contains_key(job)
}

/// Where a job's output is handed: the network, or this machine's own
/// `on`.
enum Sink<'a> {
    Stream(&'a mut Stream),
    Local(&'a Pieces),
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
    async fn ended(&mut self, end: OnEnd) -> Result<bool, String> {
        match self {
            Sink::Stream(stream) => {
                stream.say(&OnWord::Ended { end }).await?;
                match tokio::time::timeout(SILENT_FOR, stream.hear::<String>()).await {
                    Ok(Ok(word)) => Ok(word == TAKEN),
                    Ok(Err(error)) => Err(error),
                    Err(_) => Ok(false),
                }
            }
            Sink::Local(pieces) => Ok(pieces.send(Ok(OnPiece::Ended { end })).is_ok()),
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
) -> Result<bool, String> {
    let mut changes = node.jobs.subscribe();
    let mut quiet_since = Instant::now();
    loop {
        // The record first: whatever it says has ended had written its
        // last byte before, so reading the logs after it misses nothing.
        let record = crate::jobs::read(record_path).await.map_err(|failure| failure.message)?;
        let errors = record.stderr.clone();
        let mut sent = false;
        if let Some(bytes) = read_from(&record.output, out).await? {
            out += bytes.len() as u64;
            sink.bytes(false, &bytes).await?;
            sent = true;
        }
        if let Some(path) = &errors
            && let Some(bytes) = read_from(path, err).await?
        {
            err += bytes.len() as u64;
            sink.bytes(true, &bytes).await?;
            sent = true;
        }
        if sent {
            quiet_since = Instant::now();
            continue;
        }
        if let Some(end) = through(&record) {
            return sink.ended(end).await;
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
/// has ended, or it ended otherwise: an `on` ends as a subshell does.
fn through(record: &JobRecord) -> Option<OnEnd> {
    let status = record.exit.map(|exit| exit.status);
    // Output the log could not take was never handed over: the `on` did not
    // carry the job through, whatever its code.
    if let Some(lost) = record.output_lost.as_ref().or(record.stderr_lost.as_ref())
        && (record.end.is_some() || record.output_ended_at.is_some())
    {
        let error = format!("its output there stopped being kept after {} bytes: {}", lost.kept_bytes, lost.error);
        return Some(OnEnd::Otherwise { how: EndedHow::Failed { error }, status });
    }
    if let Some(end) = &record.end {
        return Some(match (&end.how, status) {
            (EndedHow::Finished, Some(status)) => OnEnd::Exited { status },
            (how, status) => OnEnd::Otherwise { how: how.clone(), status },
        });
    }
    match (status, &record.output_ended_at) {
        (Some(status), Some(_)) => Some(OnEnd::Exited { status }),
        _ => None,
    }
}

/// Up to `CHUNK` bytes of the file at `path` from `offset`, when there are
/// any.
async fn read_from(path: &str, offset: u64) -> Result<Option<Vec<u8>>, String> {
    let path = PathBuf::from(path);
    tokio::task::spawn_blocking(move || -> std::io::Result<Option<Vec<u8>>> {
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() <= offset {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut buffer = vec![0u8; CHUNK];
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
