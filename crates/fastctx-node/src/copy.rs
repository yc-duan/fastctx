//! `fastctx copy`: files from one machine to another (`fastctx_types::copy`).
//!
//! Whoever is asked to put files somewhere answers for it as itself, under
//! its own scope, since that is all the machine written to can check. So
//! the machine asked, when it holds the source, sends: it walks its path and
//! calls the destination with a data connection, and the destination admits
//! it by its scope. When it is the destination, it fetches: it calls the
//! source, which admits it by its scope and sends back over the call's data
//! connection. Asked to copy between two other machines, it has the source
//! send, so the source's scope must reach the destination.
//!
//! A source that may not act for the account cannot send to the
//! destination itself (`world-network.md`, 别人机器上的智能体什么时候能访问你的机器):
//! then the machine asked passes the copy on, fetching from the source as a
//! destination would and sending on as a source would, each piece as it
//! comes, keeping none of it.
//!
//! Over the connection go the manifest, then the destination's answer of
//! which files it needs, then those files' bytes, each checked against the
//! manifest's hash before it is put in place. The connection runs over TLS
//! to the hub, which pairs the two ends and passes the bytes on, as it sees
//! what it carries (`world-network.md`), or straight between the machines
//! over a direct link; a sent manifest is pinned besides by the digest the
//! signed call carries.
//!
//! A copy that began is not over until its machines say so
//! (`world-network.md`, 断开不是结束): whatever breaks after, the line to the
//! hub, a data connection, the machine at the other end, it is made again,
//! paced as everything that reconnects and at once when a way to its
//! machines opens, and the destination asks for each file from the byte the
//! earlier tries brought it to, so what arrived is not sent twice. One that
//! never began, as to a machine that is not online, fails at once, as `on`
//! does.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fastctx_hands::copy::{Receiver, Wanted, walk_knowing};
use fastctx_net::data::Stream;
use fastctx_net::link::{CallFailure, Feed};
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::copy::{CopyReport, EntryKind, Manifest, Place};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;
use tokio::sync::oneshot;

use crate::Node;
use crate::again::Again;
use crate::learned::Holding;

mod accepting;
pub(crate) mod update;

/// A new caller's optional stream acknowledges business admission before filesystem work.
pub(crate) async fn accept(stream: Option<Stream>) -> Result<Option<Stream>, Failure> {
    let Some(mut stream) = stream else { return Ok(None) };
    stream.say(&accepting::ACCEPTED).await.map_err(Failure::new)?;
    Ok(Some(stream))
}

/// What the destination says once it has the manifest.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum Answer {
    /// Send these entries' files, in this order; each from the byte at the
    /// same place in `from`, when it is there, or whole.
    Want {
        files: Vec<usize>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        from: Vec<u64>,
    },
    Refused {
        reason: String,
    },
}

impl Answer {
    fn want(wanted: &[Wanted], resumes: bool) -> Self {
        Answer::Want {
            files: wanted.iter().map(|wanted| wanted.index).collect(),
            from: if resumes { wanted.iter().map(|wanted| wanted.from).collect() } else { Vec::new() },
        }
    }
}

/// What came of a copy's connection once it was open: none of these is
/// the end of the copy, which goes on (`world-network.md`, 断开不是结束).
fn broke(reason: impl std::fmt::Display) -> Failure {
    Failure::unanswered(reason.to_string())
}

/// Whether a copy began, in any of its tries: one of their data connections
/// opened, or a copy asked of another machine may have reached it. From
/// then on every break is one the copy goes on through.
#[derive(Clone, Default)]
struct Began(Arc<AtomicBool>);

impl Began {
    fn now(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    fn yes(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// What a call of the copy failing says: once the copy began, that it broke
/// and goes on; before, that it could not begin.
fn ended(failure: CallFailure, began: &Began) -> Failure {
    let failure = match failure {
        CallFailure::Unknown { name, reason } if reason.starts_with("sending its data failed: ") => {
            format!("the copy with {name} stopped: {}", &reason["sending its data failed: ".len()..])
        }
        other => other.to_string(),
    };
    if began.yes() { broke(failure) } else { Failure::new(failure) }
}

/// A copy asked of `machine` that it did not answer: it may have reached
/// the machine, which copies on, so the copy began.
fn asked(failure: CallFailure, began: &Began) -> Failure {
    if matches!(failure, CallFailure::Unknown { .. } | CallFailure::Unreadable { .. }) {
        began.now();
    }
    ended(failure, began)
}

/// Copies `from` to `to`, asked of this machine by whoever works for
/// `acting`; a copy that broke on the way is made again until it is done.
/// It holds the node back from handing the World on only while a try runs:
/// between tries, a node on its way out lets it go, and asked again, the
/// node after it goes on with it.
pub async fn copy(node: &Arc<Node>, acting: &Acting, from: Place, to: Place) -> Result<CopyReport, Failure> {
    let machines: Vec<Written> =
        [from.written(), to.written()].into_iter().flatten().filter(|machine| !node.is_here(machine)).collect();
    let began = Began::default();
    let mut again = Again::default();
    let started = Instant::now();
    loop {
        let handing_over = || {
            let why = format!("the node on {} is handing over to its successor; ask again in a moment", node.machine());
            if began.yes() { broke(why) } else { Failure::new(why) }
        };
        if began.yes() && node.handoff.yielding() {
            return Err(handing_over());
        }
        let Some(busy) = node.handoff.call() else { return Err(handing_over()) };
        let tried = Instant::now();
        let failure = match Box::pin(copy_once(node, acting, from.clone(), to.clone(), &began)).await {
            Err(failure) if failure.is_unanswered() => failure,
            done => return done,
        };
        drop(busy);
        // A try that went on a while was the copy moving: the next break is
        // tried again at once, as a line that held is dialed again.
        if tried.elapsed() >= fastctx_net::again::STABLE {
            again.reset();
        }
        node.log.line(format_args!(
            "the copy of {} broke after {}: {}; going on",
            from.path,
            crate::talk::lasted(started),
            failure.message
        ));
        again.pause(node, &machines).await;
    }
}

async fn copy_once(
    node: &Arc<Node>,
    acting: &Acting,
    from: Place,
    to: Place,
    began: &Began,
) -> Result<CopyReport, Failure> {
    let here = node.machine();
    let at = |place: &Place| place.written().unwrap_or_else(|| (&here).into());
    let (from_machine, to_machine) = (at(&from), at(&to));
    let from_here = from.machine.is_none() || node.is_here(&from_machine);
    let to_here = to.machine.is_none() || node.is_here(&to_machine);
    node.settled(&from_machine).await;
    node.settled(&to_machine).await;
    node.authorize_machine(acting, &from_machine)?;
    node.authorize_machine(acting, &to_machine)?;
    if from_here && to_here {
        let root = fastctx_platform::local_path(&from.path).map_err(|error| Failure::new(error.to_string()))?;
        let target = to.path;
        let world = node.world.clone();
        return tokio::task::spawn_blocking(move || {
            fastctx_hands::copy::local::copy_protected(&root, &target, &here, world.path())
        })
        .await
        .map_err(|error| Failure::new(format!("copying {}: {error}", from.path)))?
        .map_err(|error| Failure::new(format!("copying {}: {error}", from.path)));
    }
    if !from_here && !to_here && same_machine(node, &from_machine, &to_machine)? {
        let to = Place { machine: Some(to_machine.name.clone()), hub: to_machine.hub.clone(), path: to.path };
        return asked_of(node, acting, &from_machine, from.path, to, began).await;
    }
    if from_here {
        return send(node, acting, &from.path, &to_machine, &to.path, began).await;
    }
    if to_here {
        return fetch(node, acting, &from_machine, &from.path, &to.path, began).await;
    }
    if !trusted(node, acting, &from_machine) {
        return Box::pin(relay(node, acting, &from_machine, &from.path, &to_machine, &to.path, began)).await;
    }
    let to = Place { machine: Some(to_machine.name), hub: to_machine.hub, path: to.path };
    asked_of(node, acting, &from_machine, from.path, to, began).await
}

/// Asks `source` to send `from` to `to` itself.
async fn asked_of(
    node: &Node,
    acting: &Acting,
    source: &Written,
    from: String,
    to: Place,
    began: &Began,
) -> Result<CopyReport, Failure> {
    accepting::ask(node, acting, source, from, to, began).await
}

fn same_machine(node: &Node, from: &Written, to: &Written) -> Result<bool, Failure> {
    if from == to {
        return Ok(true);
    }
    if from.name != to.name {
        return Ok(false);
    }
    let sources = node.locate(from)?;
    let destinations = node.locate(to)?;
    Ok(sources
        .iter()
        .any(|source| destinations.iter().any(|destination| source.membership().id == destination.membership().id)))
}

/// Whether `machine` may act toward other machines for an account `acting`
/// works for: one logged in on it, or that trusts it. When `acting` may not
/// use it at all, the copy is asked of it all the same, and its refusal says
/// why.
fn trusted(node: &Node, acting: &Acting, machine: &Written) -> bool {
    match node.speakers(acting, machine, Purpose::Hands) {
        Ok(speakers) => speakers.iter().any(|(link, account)| link.view().may_act_for(account, &machine.name)),
        Err(_) => true,
    }
}

/// A feed that hands its end of a data connection to whoever waits on
/// `slot`, once.
fn handing(slot: Arc<Mutex<Option<oneshot::Sender<Stream>>>>) -> impl Fn() -> Feed {
    move || -> Feed {
        let slot = slot.clone();
        Box::new(move |stream| {
            Box::pin(async move {
                let sender = slot
                    .lock()
                    .expect("never poisoned")
                    .take()
                    .ok_or_else(|| "this end of the copy was already taken".to_string())?;
                sender.send(stream).map_err(|_| "the copy stopped passing on".to_string())
            })
        })
    }
}

/// Copies `from` on `from_machine` to `to` on `to_machine`, both other
/// machines, through this one, as each piece comes.
async fn relay(
    node: &Arc<Node>,
    acting: &Acting,
    from_machine: &Written,
    from: &str,
    to_machine: &Written,
    to: &str,
    began: &Began,
) -> Result<CopyReport, Failure> {
    if let Some(failure) = offline(node, to_machine) {
        return Err(ended(failure, began));
    }
    let (giving, given) = oneshot::channel();
    let (source_written, destination_written) = (from_machine.clone(), to_machine.clone());
    let call = RemoteCall::CopyFetch { from: from.to_string() };
    // Each of the two calls is a large future; on the heap, the relay does
    // not overflow a worker's stack.
    let fetching = Box::pin(node.call_with_stream(
        acting,
        &source_written,
        Purpose::Hands,
        call,
        handing(Arc::new(Mutex::new(Some(giving)))),
    ));
    let passing = Box::pin(async {
        let mut source = given.await.map_err(|_| Failure::new(format!("{from_machine} did not send the copy")))?;
        began.now();
        let offer: Offer = source.hear_step("offer").await.map_err(broke)?;
        let mut manifest = Vec::new();
        source
            .take(offer.manifest_bytes, |piece| {
                manifest.extend_from_slice(piece);
                Ok(())
            })
            .await
            .map_err(broke)?;
        let entries: Manifest = serde_json::from_slice(&manifest)
            .map_err(|error| Failure::new(format!("the manifest from {from_machine} cannot be read: {error}")))?;
        let call = RemoteCall::CopyReceive {
            to: to.to_string(),
            manifest_bytes: manifest.len() as u64,
            manifest_sha256: format!("{:x}", Sha256::digest(&manifest)),
            resumes: offer.resumes,
        };
        let (taking, taken) = oneshot::channel();
        let receiving = Box::pin(node.call_with_stream(
            acting,
            &destination_written,
            Purpose::Hands,
            call,
            handing(Arc::new(Mutex::new(Some(taking)))),
        ));
        let piping = async move {
            let mut destination = taken.await.map_err(|_| format!("{to_machine} did not take the copy"))?;
            destination.send(&manifest).await?;
            let answer: Answer = destination.hear_step("requested files").await?;
            source.say(&answer).await?;
            let Answer::Want { files, from } = answer else { return Ok(()) };
            for (at, index) in files.into_iter().enumerate() {
                let Some(EntryKind::File { size, .. }) = entries.entries.get(index).map(|entry| &entry.kind) else {
                    return Err(format!("{to_machine} asked for entry {index}, which is not a file of this copy"));
                };
                let from = from.get(at).copied().unwrap_or(0);
                source.forward(&mut destination, size.saturating_sub(from)).await?;
            }
            source.done().await?;
            destination.finish().await
        };
        let (received, piped) = tokio::join!(receiving, piping);
        let received = match received {
            Ok(Ok(RemoteReply::Copied(report))) => Ok(report),
            Ok(Ok(_)) => Err(Failure::new(format!("{to_machine} answered something other than a copy"))),
            Ok(Err(failure)) => Err(failure),
            Err(failure) => Err(ended(failure, began)),
        };
        match (received, piped) {
            (Ok(report), _) => Ok(report),
            (Err(failure), Ok(())) => Err(failure),
            (Err(failure), Err(why)) => Err(broke(format!("{}; passing it on: {why}", failure.message))),
        }
    });
    let (fetched, passed) = tokio::join!(fetching, passing);
    let sent = match fetched {
        Ok(Ok(RemoteReply::Copied(report))) => Ok(report),
        Ok(Ok(_)) => Err(Failure::new(format!("{from_machine} answered something other than a copy"))),
        Ok(Err(failure)) => Err(failure),
        Err(failure) => Err(ended(failure, began)),
    };
    match (sent, passed) {
        (Ok(sent), Ok(mut report)) => {
            report.from = sent.from;
            report.from_machine = sent.from_machine;
            let mut problems = sent.problems;
            problems.append(&mut report.problems);
            report.problems = problems;
            Ok(report)
        }
        // Everything reached the destination, which checked it.
        (Err(failure), Ok(mut report)) => {
            report.from = from.to_string();
            report.from_machine = from_machine.name.clone();
            report.problems.push(format!("{from_machine}, which sent it, said afterwards: {}", failure.message));
            Ok(report)
        }
        (Ok(_), Err(failure)) => Err(failure),
        (Err(failure), Err(passing)) => {
            let message = format!("{}; {}", failure.message, passing.message);
            Err(if failure.is_unanswered() || passing.is_unanswered() { broke(message) } else { Failure::new(message) })
        }
    }
}

/// What a source has to send: its manifest, the files behind it, what it
/// could not read, and its hold on what it read, for a try after this one.
struct Walked<'a> {
    manifest: Vec<u8>,
    files: Vec<(PathBuf, Option<u64>)>,
    problems: Vec<String>,
    holding: Holding<'a>,
}

/// Walks `from`, on this machine, for sending; what an earlier try read of
/// it, and is as it was then, is not read again (`learned`).
async fn walked<'a>(node: &'a Node, from: &str) -> Result<Walked<'a>, Failure> {
    let root = fastctx_platform::local_path(from).map_err(|error| Failure::new(error.to_string()))?;
    if !root.is_absolute() {
        return Err(Failure::new(format!("{from} is not an absolute path on {}", node.machine())));
    }
    let holding = node.learned.hold(&root);
    let known = holding.known.clone();
    let walked = tokio::task::spawn_blocking(move || walk_knowing(&root, &mut known.lock().expect("never poisoned")))
        .await
        .map_err(|error| Failure::new(format!("reading {from}: {error}")))?
        .map_err(|error| Failure::new(format!("{from} on {}: {error}", node.machine())))?;
    let manifest = serde_json::to_vec(&walked.manifest).expect("manifests serialize");
    let files = walked
        .sources
        .iter()
        .zip(&walked.manifest.entries)
        .map(|(path, entry)| match entry.kind {
            EntryKind::File { size, .. } => (path.clone(), Some(size)),
            _ => (path.clone(), None),
        })
        .collect();
    Ok(Walked { manifest, files, problems: walked.problems, holding })
}

/// What a source that is fetched from says first: how long its manifest
/// is, and whether it sends a file from a byte the destination names.
#[derive(Serialize, Deserialize)]
struct Offer {
    manifest_bytes: u64,
    #[serde(default)]
    resumes: bool,
}

/// The source's side when it is fetched from: walks `from`, here, and sends
/// it back over the caller's data connection.
pub async fn give(node: &Arc<Node>, from: &str, mut stream: Stream) -> Result<CopyReport, Failure> {
    // Reading every file for the manifest is work toward the offer the
    // other end waits for.
    let walking = stream.settling();
    let Walked { manifest, files, problems, holding: _held } = walked(node, from).await?;
    drop(walking);
    stream.say(&Offer { manifest_bytes: manifest.len() as u64, resumes: true }).await.map_err(broke)?;
    feed(stream, manifest, files).await.map_err(broke)?;
    Ok(CopyReport {
        from: slashed(Path::new(from)),
        from_machine: node.machine(),
        to: String::new(),
        to_machine: node.machine(),
        files: 0,
        bytes: 0,
        directories: 0,
        links: 0,
        unchanged: 0,
        problems,
    })
}

/// The destination's side when it is the machine asked: calls `from_machine`
/// for `from` and puts it at `to`, here.
async fn fetch(
    node: &Arc<Node>,
    acting: &Acting,
    from_machine: &Written,
    from: &str,
    to: &str,
    began: &Began,
) -> Result<CopyReport, Failure> {
    // The feed keeps what it took before it sends its receipt; the source
    // answers only after the receipt, so the answer never comes first.
    let taken: Arc<std::sync::Mutex<Option<CopyReport>>> = Arc::default();
    let feed = || -> Feed {
        let (here, to, kept, began) = (node.clone(), to.to_string(), taken.clone(), began.clone());
        Box::new(move |mut stream| {
            Box::pin(async move {
                began.now();
                let offer: Offer = stream.hear_step("offer").await?;
                let report = take_files(&here, to, offer.manifest_bytes, None, offer.resumes, &mut stream)
                    .await
                    .map_err(|failure| failure.message)?;
                *kept.lock().expect("never poisoned") = Some(report);
                stream.done().await
            })
        })
    };
    let call = RemoteCall::CopyFetch { from: from.to_string() };
    let answer = node.call_with_stream(acting, from_machine, Purpose::Hands, call, feed).await;
    let answer = answer.map_err(|failure| ended(failure, began))?;
    let sent = match answer {
        Ok(RemoteReply::Copied(report)) => report,
        Ok(_) => return Err(Failure::new(format!("{from_machine} answered something other than a copy"))),
        Err(failure) => return Err(failure),
    };
    let mut report = taken
        .lock()
        .expect("never poisoned")
        .take()
        .ok_or_else(|| Failure::new(format!("{from_machine} answered before the copy had arrived")))?;
    report.from = sent.from;
    report.from_machine = sent.from_machine;
    let mut problems = sent.problems;
    problems.append(&mut report.problems);
    report.problems = problems;
    Ok(report)
}

/// The source's side: walks `from`, here, and sends it to `to` on `to_machine`.
async fn send(
    node: &Arc<Node>,
    acting: &Acting,
    from: &str,
    to_machine: &Written,
    to: &str,
    began: &Began,
) -> Result<CopyReport, Failure> {
    // A destination the hub says is not connected is said so before the
    // source is read, as `on` says it before it runs (`world-run.md`).
    if let Some(failure) = offline(node, to_machine) {
        return Err(ended(failure, began));
    }
    let Walked { manifest, files, problems: walk_problems, holding: _held } = walked(node, from).await?;
    let call = RemoteCall::CopyReceive {
        to: to.to_string(),
        manifest_bytes: manifest.len() as u64,
        manifest_sha256: format!("{:x}", Sha256::digest(&manifest)),
        resumes: true,
    };
    let sending = || -> Feed {
        let (manifest, files, began) = (manifest.clone(), files.clone(), began.clone());
        Box::new(move |stream| {
            began.now();
            Box::pin(feed(stream, manifest, files))
        })
    };
    let answer = node.call_with_stream(acting, to_machine, Purpose::Hands, call, sending).await;
    let answer = answer.map_err(|failure| ended(failure, began))?;
    let mut report = match answer {
        Ok(RemoteReply::Copied(report)) => report,
        Ok(_) => return Err(Failure::new(format!("{to_machine} answered something other than a copy"))),
        Err(failure) => return Err(failure),
    };
    report.from = slashed(Path::new(from));
    report.from_machine = node.machine();
    let mut problems = walk_problems;
    problems.append(&mut report.problems);
    report.problems = problems;
    Ok(report)
}

/// That `machine` is not connected, when every hub it is on says so while
/// this machine's line to that hub is up and nothing links the two
/// directly; then a call to it would come back with just that.
fn offline(node: &Node, machine: &Written) -> Option<CallFailure> {
    let mut last_seen = None;
    for link in node.locate(machine).ok()? {
        let member = link.view().member(&machine.name).cloned()?;
        let heard = matches!(link.status(), fastctx_net::link::Status::Online { .. });
        if member.online || !heard || link.way_to(&machine.name).is_some() {
            return None;
        }
        last_seen = last_seen.max(member.since);
    }
    Some(CallFailure::Offline { name: machine.name.clone(), last_seen })
}

/// A path as the World writes it: on Windows with forward slashes.
fn slashed(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) { text.replace('\\', "/") } else { text.into_owned() }
}

/// The source's end of the data connection: the manifest, then the files
/// the destination wants.
async fn feed(mut stream: Stream, manifest: Vec<u8>, files: Vec<(PathBuf, Option<u64>)>) -> Result<(), String> {
    stream.send(&manifest).await?;
    let (wanted, from) = match stream.hear_step::<Answer>("requested files").await? {
        Answer::Want { files, from } => (files, from),
        Answer::Refused { reason } => return Err(reason),
    };
    let mut buffer = vec![0u8; 1 << 20];
    for (at, index) in wanted.into_iter().enumerate() {
        let Some((path, Some(size))) = files.get(index) else {
            return Err(format!("the destination asked for entry {index}, which is not a file of this copy"));
        };
        send_file(&mut stream, path, *size, from.get(at).copied().unwrap_or(0), &mut buffer).await?;
    }
    stream.finish().await
}

/// Sends exactly the bytes of a file from `from` to `size`. A file that has
/// shrunk since it was hashed, or cannot be read, is padded with zeros to
/// its size, so the transfer stays in step; its hash then shows the
/// destination it is not the file promised, and it is not kept.
async fn send_file(stream: &mut Stream, path: &Path, size: u64, from: u64, buffer: &mut [u8]) -> Result<(), String> {
    use tokio::io::AsyncSeekExt as _;
    let mut left = size.saturating_sub(from);
    let opened = match tokio::fs::File::open(path).await {
        Ok(mut file) => file.seek(std::io::SeekFrom::Start(from)).await.map(|_| file).ok(),
        Err(_) => None,
    };
    if let Some(mut file) = opened {
        while left > 0 {
            let want = (buffer.len() as u64).min(left) as usize;
            match file.read(&mut buffer[..want]).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    stream.send(&buffer[..read]).await?;
                    left -= read as u64;
                }
            }
        }
    }
    buffer.fill(0);
    while left > 0 {
        let want = (buffer.len() as u64).min(left) as usize;
        stream.send(&buffer[..want]).await?;
        left -= want as u64;
    }
    Ok(())
}

/// The destination's side when the source calls: takes the copy, and says
/// it has everything.
pub async fn receive(
    node: &Node,
    to: String,
    manifest_bytes: u64,
    manifest_sha256: String,
    resumes: bool,
    mut stream: Stream,
) -> Result<CopyReport, Failure> {
    let report = take_files(node, to, manifest_bytes, Some(&manifest_sha256), resumes, &mut stream).await?;
    stream.done().await.map_err(broke)?;
    Ok(report)
}

/// Takes the manifest, checked against `manifest_sha256` when the source
/// signed for one, says which files it needs, from where the earlier tries
/// brought each when the source `resumes`, and puts each in place as it
/// comes; the receipt is the caller's to send.
async fn take_files(
    node: &Node,
    to: String,
    manifest_bytes: u64,
    manifest_sha256: Option<&str>,
    resumes: bool,
    stream: &mut Stream,
) -> Result<CopyReport, Failure> {
    let failure = |reason: String| broke(reason);
    let mut json = Vec::new();
    stream
        .take(manifest_bytes, |piece| {
            json.extend_from_slice(piece);
            Ok(())
        })
        .await
        .map_err(failure)?;
    if manifest_sha256.is_some_and(|signed| format!("{:x}", Sha256::digest(&json)) != signed) {
        return Err(Failure::new("the manifest is not the one the source signed for"));
    }
    let manifest: Manifest =
        serde_json::from_slice(&json).map_err(|error| Failure::new(format!("the manifest cannot be read: {error}")))?;
    let receiver = match Receiver::new(&to, manifest).and_then(|receiver| receiver.protect(node.world.path())) {
        Ok(receiver) => receiver,
        Err(reason) => {
            let _ = stream.say(&Answer::Refused { reason: reason.clone() }).await;
            return Err(Failure::new(reason));
        }
    };
    // One copy at a time writes a destination: the try before this one may
    // still be taking what was on its way when it broke, and two adding to
    // the same file's part would mix their bytes. A try that broke ends
    // once its connection is heard to be gone.
    let _writing = node.copying.lock(receiver.destination()).await;
    // What earlier tries read of what is there, and what they left of a
    // file that arrived in part.
    let holding = node.learned.hold(receiver.destination());
    // Looking at what is there already reads every file it has, but those
    // an earlier try read and that are as they were then.
    let known = holding.known.clone();
    // That reading is work toward the answer the other end waits for.
    let looking = stream.settling();
    let (mut receiver, wanted) = blocking(move || {
        let _looking = looking;
        let mut receiver = receiver;
        let wanted = receiver.wanted(resumes, &mut known.lock().expect("never poisoned"));
        (receiver, wanted)
    })
    .await?;
    stream.say(&Answer::want(&wanted, resumes)).await.map_err(failure)?;
    for Wanted { index, from } in wanted {
        let from = if resumes { from } else { 0 };
        // Going on with a file may read what is here of it.
        let known = holding.known.clone();
        let preparing = stream.settling();
        let (back, (left, arriving)) = blocking(move || {
            let _preparing = preparing;
            let arrival = receiver.arrive(index, from, &mut known.lock().expect("never poisoned"));
            (receiver, arrival)
        })
        .await?;
        receiver = back;
        let work = stream.receipt_work();
        let (arriving, taken) =
            stream.take_writing(left, arriving, |arriving, piece| arriving.write(piece)).await.map_err(failure)?;
        // Putting a file in place makes it lasting; one the copy broke off
        // in is kept as it stands, for the try after this one.
        let (known, broke_off) = (holding.known.clone(), taken.is_err());
        let settling = work.begin();
        receiver = blocking(move || {
            let _settling = settling;
            let known = &mut known.lock().expect("never poisoned");
            if broke_off {
                receiver.broke(index, arriving, known);
            } else {
                receiver.arrived(index, arriving, known);
            }
            receiver
        })
        .await?;
        taken.map_err(failure)?;
    }
    let settling = stream.settling();
    let receiver = blocking(move || {
        let _settling = settling;
        receiver.link();
        receiver
    })
    .await?;
    Ok(CopyReport {
        from: String::new(),
        from_machine: node.machine(),
        to: slashed(receiver.destination()),
        to_machine: node.machine(),
        files: receiver.files,
        bytes: receiver.bytes,
        directories: receiver.directories,
        links: receiver.links,
        unchanged: receiver.unchanged,
        problems: receiver.problems,
    })
}

/// Runs `work`, which reads and writes files, off the runtime's threads.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work).await.map_err(|error| Failure::new(format!("at the destination: {error}")))
}
