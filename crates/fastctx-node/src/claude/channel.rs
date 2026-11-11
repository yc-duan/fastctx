//! The loopback channel between a Claude session's mod and this node
//! (`dev-notes/skel/mod-channel-v1.md`): the mod hands its reports over in
//! memory, as they come, and waits on a request the node answers the moment
//! it leaves the session something (words, a stop, a control), instead of
//! files a helper process watches and the node scans. The files stay: a mod
//! that cannot reach this, or one loaded before it, goes on with them.
//!
//! The node listens on `127.0.0.1` at a port of the system's choosing (a
//! named pipe, the other way on Windows, is refused by Claude Code), and
//! tells each session where in its directory's `channel.json`, with a token
//! for that session alone: a process that can read the session's directory
//! could write its files as well, and no other user's process gets in.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::Node;

/// How long a wait is held before it is answered with nothing new.
const WAIT: Duration = Duration::from_secs(25);
/// The most one handing over of reports may hold.
const BODY_MOST: usize = 64 * 1024 * 1024;
/// What the channel says of itself in `channel.json`.
const VERSION: u32 = 1;

pub struct Channel {
    port: u16,
    /// Made anew each time the node starts: a session's token changes with
    /// it, and its mod reads `channel.json` again when a request is refused.
    key: [u8; 32],
    wakes: Mutex<HashMap<String, Arc<Wake>>>,
    /// The sessions told where the channel is since the node started.
    told: Mutex<HashSet<String>>,
    queues: Mutex<HashMap<String, Arc<Queue>>>,
}

/// One session's reports handed over and not yet applied, in the order they
/// came: they are taken at once, so what the mod does next waits for no
/// report to be applied, and applied in this order by one task of their own.
struct Queue {
    sender: tokio::sync::mpsc::UnboundedSender<Vec<HandedReport>>,
    /// The last report dealt with: applied, or written back to the session's
    /// files to be applied there. Its mod keeps every report after it, to
    /// write to files itself should the node go.
    through: Mutex<Option<String>>,
}

/// How many times the node has left a session something since it started,
/// and those waiting to hear of the next time.
struct Wake {
    count: AtomicU64,
    changed: tokio::sync::Notify,
}

impl Channel {
    fn token(&self, session: &str) -> String {
        let mut hash = Sha256::new();
        hash.update(self.key);
        hash.update(b":");
        hash.update(session.as_bytes());
        hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn wake_of(&self, session: &str) -> Arc<Wake> {
        self.wakes
            .lock()
            .expect("never poisoned")
            .entry(session.to_string())
            .or_insert_with(|| Arc::new(Wake { count: AtomicU64::new(0), changed: tokio::sync::Notify::new() }))
            .clone()
    }

    /// Answers once the session has been left something since `seen`, or
    /// after `WAIT` with nothing new: how many times it has been.
    async fn wait(&self, session: &str, seen: u64) -> u64 {
        let wake = self.wake_of(session);
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let changed = wake.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let count = wake.count.load(Ordering::Acquire);
            if count != seen {
                return count;
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return wake.count.load(Ordering::Acquire);
            }
        }
    }
}

/// Starts listening, before any session is looked at.
pub(super) fn start(node: &Arc<Node>) -> io::Result<()> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(io::Error::other)?;
    let channel =
        Arc::new(Channel { port, key, wakes: Mutex::default(), told: Mutex::default(), queues: Mutex::default() });
    if node.claude.channel.set(channel).is_err() {
        return Ok(());
    }
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                node.log.line(format_args!("the Claude sessions' loopback channel did not start: {error}"));
                return;
            }
        };
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let _ = stream.set_nodelay(true);
                    fastctx_platform::task::spawn(serve(node.clone(), stream));
                }
                Err(error) => {
                    node.log.line(format_args!("accepting on the Claude sessions' loopback channel: {error}"));
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });
    Ok(())
}

/// Tells the session in `directory` where the channel is, once each time the
/// node starts (and again whenever its mod reserves the directory).
pub(crate) fn tell(node: &Node, directory: &Path, session: &str, again: bool) {
    let Some(channel) = node.claude.channel.get() else { return };
    if !again && channel.told.lock().expect("never poisoned").contains(session) {
        return;
    }
    let said = json!({"version": VERSION, "port": channel.port, "token": channel.token(session)});
    let path = directory.join("channel.json");
    let bytes = serde_json::to_vec(&said).expect("JSON serializes");
    let same = fastctx_platform::disk::read(&path).is_ok_and(|current| current == bytes);
    // Where the channel is changes with the node: a lost power takes nothing
    // that the node's next start does not write again.
    let written = same || fastctx_platform::publish_unsynced(&path, &bytes, fastctx_platform::Permissions::Private).is_ok();
    if written {
        channel.told.lock().expect("never poisoned").insert(session.to_string());
    }
}

/// The node left the session something its mod takes when it looks (words,
/// a stop, a control): a mod waiting on the channel hears of it now.
pub(crate) fn wake(node: &Node, session: &str) {
    if let Some(channel) = node.claude.channel.get() {
        let wake = channel.wake_of(session);
        wake.count.fetch_add(1, Ordering::AcqRel);
        wake.changed.notify_waiters();
    }
}

async fn serve(node: Arc<Node>, stream: tokio::net::TcpStream) {
    let service = hyper::service::service_fn(move |request| {
        let node = node.clone();
        async move { Ok::<_, Infallible>(respond(&node, request).await) }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .await;
}

async fn respond(node: &Arc<Node>, request: Request<Incoming>) -> Response<Full<Bytes>> {
    match route(node, request).await {
        Ok(answer) => reply(StatusCode::OK, &answer),
        Err((status, why)) => reply(status, &json!({ "error": why })),
    }
}

fn reply(status: StatusCode, body: &Value) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(serde_json::to_vec(body).expect("JSON serializes"))));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(hyper::header::CONTENT_TYPE, hyper::header::HeaderValue::from_static("application/json"));
    response
}

type Refusal = (StatusCode, String);

async fn route(node: &Arc<Node>, request: Request<Incoming>) -> Result<Value, Refusal> {
    let channel = node.claude.channel.get().ok_or((StatusCode::SERVICE_UNAVAILABLE, "starting".to_string()))?;
    let path = request.uri().path().to_string();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let [version, "claude", session, what] = parts[..] else {
        return Err((StatusCode::NOT_FOUND, format!("no such place: {path}")));
    };
    if version != "v1" {
        return Err((StatusCode::NOT_FOUND, format!("no channel version {version}")));
    }
    if session.is_empty() || !session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err((StatusCode::BAD_REQUEST, "not a session's name".to_string()));
    }
    let expected = format!("Bearer {}", channel.token(session));
    let given = request.headers().get(hyper::header::AUTHORIZATION).map(|value| value.as_bytes()).unwrap_or_default();
    if !same_bytes(given, expected.as_bytes()) {
        return Err((StatusCode::UNAUTHORIZED, "not this session's token".to_string()));
    }
    match (request.method().clone(), what) {
        (Method::GET, "wake") => {
            let seen = request
                .uri()
                .query()
                .and_then(|query| query.split('&').find_map(|pair| pair.strip_prefix("seen=")))
                .and_then(|seen| seen.parse().ok())
                .unwrap_or(0);
            Ok(json!({ "wake": channel.wait(session, seen).await }))
        }
        (Method::POST, "reports") => {
            let body = Limited::new(request.into_body(), BODY_MOST)
                .collect()
                .await
                .map_err(|error| (StatusCode::BAD_REQUEST, format!("reading the reports: {error}")))?
                .to_bytes();
            let handed: Handed = serde_json::from_slice(&body)
                .map_err(|error| (StatusCode::BAD_REQUEST, format!("reading the reports: {error}")))?;
            let names: Vec<String> = handed.reports.iter().map(|report| report.name.clone()).collect();
            if let Some(name) = names.iter().find(|name| !name.strip_suffix(".json").is_some_and(watermark)) {
                return Err((StatusCode::BAD_REQUEST, format!("{name} is not a report's name")));
            }
            let queue = queue(node, channel, session);
            queue
                .sender
                .send(handed.reports)
                .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "the session's reports are not taken now".to_string()))?;
            let through = queue.through.lock().expect("never poisoned").clone();
            Ok(json!({ "received": names, "applied": through }))
        }
        (Method::GET, "subagents") => {
            let configuration = crate::profile::subagents::for_session(node, session)
                .await
                .map_err(|failure| (StatusCode::CONFLICT, failure.message))?;
            Ok(json!({ "configuration": configuration }))
        }
        (method, what) => Err((StatusCode::NOT_FOUND, format!("no {method} {what}"))),
    }
}

#[derive(serde::Deserialize)]
struct Handed {
    reports: Vec<HandedReport>,
}

/// The session's queue, with the task that applies it started the first time.
fn queue(node: &Arc<Node>, channel: &Channel, session: &str) -> Arc<Queue> {
    let mut queues = channel.queues.lock().expect("never poisoned");
    if let Some(queue) = queues.get(session) {
        return queue.clone();
    }
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let queue = Arc::new(Queue { sender, through: Mutex::new(None) });
    queues.insert(session.to_string(), queue.clone());
    fastctx_platform::task::spawn(apply_in_order(node.clone(), session.to_string(), queue.clone(), receiver));
    queue
}

/// Applies a session's handed reports in the order they came, those that
/// came meanwhile together. What cannot be applied (a session not known
/// yet, a report that failed) is written to the session's files, as its mod
/// would have written it, and applied by the next look as files are.
async fn apply_in_order(
    node: Arc<Node>,
    session: String,
    queue: Arc<Queue>,
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<Vec<HandedReport>>,
) {
    let directory = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(&session);
    while let Some(mut batch) = receiver.recv().await {
        while let Ok(more) = receiver.try_recv() {
            batch.extend(more);
        }
        let Some(last) = batch.last().map(|report| report.name.clone()) else { continue };
        let kept: Vec<(String, Value)> = batch.iter().map(|report| (report.name.clone(), report.report.clone())).collect();
        let reports = batch.iter().map(|report| (report.name.clone(), report.report.clone())).collect();
        let applied = match super::receive(&node, &session, reports).await {
            Ok(applied) => applied,
            Err(error) => {
                node.log.line(format_args!("applying what {session} handed over; to its files instead: {error}"));
                Vec::new()
            }
        };
        // A batch's last fragment is named after its file's name: the mod
        // reads where its names stand from these markers when it starts again
        // (`startSession`), as from the files it wrote.
        for report in &batch {
            let Some(sequence) = report.sequence.as_deref() else { continue };
            if applied.contains(&report.name) && watermark(sequence) {
                let marker = directory.join("applied").join(format!("{sequence}.sequence"));
                match fastctx_platform::publish_new_unsynced(&marker, b"", fastctx_platform::Permissions::Private) {
                    Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                        node.log.line(format_args!("keeping where {session}'s report names stand: {error}"));
                    }
                    _ => {}
                }
            }
        }
        let rest: Vec<(String, Value)> = kept.into_iter().filter(|(name, _)| !applied.contains(name)).collect();
        if !rest.is_empty() {
            let (writing, count) = (directory.clone(), rest.len());
            let written = fastctx_platform::task::spawn_blocking(move || to_files(&writing, &rest)).await;
            if !matches!(written, Ok(Ok(()))) {
                node.log.line(format_args!("writing {count} of {session}'s handed reports to its files failed"));
            }
        }
        *queue.through.lock().expect("never poisoned") = Some(last);
    }
}

/// Writes reports to the session's files as its mod writes them: the report,
/// then its marker that it is whole.
fn to_files(directory: &Path, reports: &[(String, Value)]) -> io::Result<()> {
    let events = directory.join("events");
    fastctx_platform::ensure_private_dir(&events)?;
    for (name, report) in reports {
        let path = events.join(name);
        fastctx_platform::publish(&path, &serde_json::to_vec(report)?, fastctx_platform::Permissions::Private)?;
        fastctx_platform::publish(&path.with_extension("done"), b"", fastctx_platform::Permissions::Private)?;
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct HandedReport {
    name: String,
    /// The name taken when a batch of streamed observations was sealed.
    #[serde(default)]
    sequence: Option<String>,
    report: Value,
}

/// Whether `name` is a mod's report name: its clock and a count, at the
/// widths it writes them.
fn watermark(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 22 && bytes[..15].iter().all(u8::is_ascii_digit) && bytes[15] == b'-' && bytes[16..].iter().all(u8::is_ascii_digit)
}

/// Whether two byte strings are equal, taking as long whichever byte differs.
fn same_bytes(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && left.iter().zip(right).fold(0u8, |differ, (a, b)| differ | (a ^ b)) == 0
}
