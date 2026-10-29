//! User-only line sources, owned by live connections and the holding file.

mod projection;
pub(crate) mod reading;

use fastctx_types::access::Written;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};

use fastctx_hands::lines::NotGrown;
use fastctx_types::MachineName;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::lines::{Decoding, Extent, Reply, Request, Result as Answer};
use fastctx_types::remote::{RemoteCall, RemoteReply};

use crate::Node;

type Key = (MachineName, String);
#[derive(Default)]
pub(crate) struct Sources(Mutex<HashMap<Key, Arc<Source>>>);
struct Source {
    path: PathBuf,
    immutable: bool,
    annotation: Option<String>,
    job_record: Option<PathBuf>,
    notes_record: Option<PathBuf>,
    state: Mutex<State>,
    changed: tokio::sync::watch::Sender<u64>,
    closed: tokio::sync::watch::Sender<bool>,
    events: tokio::sync::watch::Sender<std::result::Result<u64, String>>,
    _watcher: Mutex<fastctx_watch::Watcher>,
    cancelled: Arc<AtomicBool>,
    requests: Mutex<Vec<Weak<AtomicBool>>>,
}
struct State {
    index: Option<fastctx_hands::lines::Index>,
    snapshot: fastctx_platform::file_read::Snapshot,
    extent: Extent,
    first: u64,
    error: Option<String>,
    /// The last look failed for a passing reason: it is looked at again soon.
    stale: bool,
    /// The first line a growth changed that no change has told yet: one
    /// whose later steps failed is told with the next.
    unsaid: Option<u64>,
    projection: Option<fastctx_hands::run::projection::CapturedNotes>,
    /// A refresh of the notes failed part way: they are read afresh at the
    /// next look, and shown as they were until then.
    notes_broken: bool,
    writes: Vec<fastctx_types::run_facts::WriteFact>,
    writes_revision: Option<(u64, std::time::SystemTime)>,
}
pub(crate) enum Lease {
    Here { node: Weak<Node>, key: Key, closed: tokio::sync::watch::Receiver<bool> },
    Remote(fastctx_net::lines::Lease),
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Self::Here { node, key, .. } = self
            && let Some(node) = node.upgrade()
        {
            close(&node, key);
        }
    }
}
impl Lease {
    pub(crate) async fn closed(&mut self) {
        match self {
            Self::Here { closed, .. } => {
                let _ = closed.wait_for(|closed| *closed).await;
            }
            Self::Remote(lease) => lease.closed().await,
        }
    }
}
struct Cancel(Arc<AtomicBool>);
impl Default for Cancel {
    fn default() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
}
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub(crate) async fn open(
    node: &Arc<Node>,
    from: MachineName,
    machine: Option<Written>,
    path: String,
    decoding: Decoding,
) -> std::result::Result<(Reply, Lease), Failure> {
    if let Some(machine) = machine.filter(|machine| !node.is_here(machine)) {
        let (link, account) = node.speaking(&Acting::Here, &machine, Purpose::Hands).map_err(failed)?;
        let (reply, lease) = fastctx_net::lines::open(link, machine.name, account, path, decoding).await?;
        return Ok((reply, Lease::Remote(lease)));
    }
    let initializing = crate::initializing::take()?;
    let (decoding, expected, policy, annotation) = match decoding {
        Decoding::Immutable { content, policy, annotation, decoding } => {
            if matches!(*decoding, Decoding::Immutable { .. }) {
                return Err(failed("an immutable line decoding cannot be nested"));
            }
            (*decoding, Some(content), policy, annotation)
        }
        decoding => (decoding, None, None, None),
    };
    let immutable = expected.is_some();
    if policy.is_some_and(|policy| policy != fastctx_hands::lines::output_policy()) {
        return Err(failed("the output decoding changed since its history was read"));
    }
    let path = fastctx_platform::local_path(&path).map_err(failed)?;
    if !path.is_absolute() {
        return Err(failed("the text path must be absolute"));
    }
    let (job_record, notes_record) = match &decoding {
        Decoding::Output { job } => {
            let found = crate::look::look_here(node, job.as_str()).await?.0;
            let Some(fastctx_types::town::Looked::Job { record, .. }) = found else {
                return Err(Failure::output_not_retained("the output job is no longer retained"));
            };
            let expected = PathBuf::from(&record.output);
            if path != expected && record.stderr.as_ref().is_none_or(|stderr| std::path::Path::new(stderr) != path) {
                return Err(failed("the path is not this job's output"));
            }
            let job_record = node.world.job_owner(&node.names(), &record.owner).job_record(&record.id);
            let notes_record = (path == expected).then(|| job_record.clone());
            (Some(job_record), notes_record)
        }
        Decoding::File { .. } => (None, None),
        Decoding::Immutable { .. } => unreachable!("unwrapped immutable decoding"),
    };
    let (events, event_rx) = tokio::sync::watch::channel(Ok(0u64));
    let notices = events.clone();
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        if !crate::file_events::may_change(&event) {
            return;
        }
        // A watch that failed may have missed a change: it counts as one.
        notices.send_modify(|state| {
            if let Ok(version) = state {
                *version += 1;
            }
        });
    });
    watcher
        .watch(
            path.parent().ok_or_else(|| failed("the source has no parent directory"))?,
            notify::RecursiveMode::NonRecursive,
        )
        .map_err(|error| watch_failed(error, &path))?;
    if let Some(record) = &job_record
        && record.parent() != path.parent()
    {
        watcher
            .watch(record.parent().expect("job parent"), notify::RecursiveMode::NonRecursive)
            .map_err(|error| watch_failed(error, &path))?;
    }
    let cancelled = Cancel::default();
    let cancel = cancelled.0.clone();
    let (read_path, directory, record_path) = (path.clone(), node.world.path().to_path_buf(), job_record.clone());
    let note_path = notes_record.clone();
    let expected_annotation = annotation.clone();
    let opened = initializing
        .run(move || {
            let snapshot = observe(&read_path).map_err(source_failed)?;
            if let Some(expected) = expected
                && fastctx_hands::files::content_fingerprint(&read_path).map_err(failed)? != expected
            {
                return Err(failed("the artifact no longer matches the history's content fingerprint"));
            }
            let (output, selection) = match &decoding {
                Decoding::File { encoding } => (false, encoding.as_deref()),
                Decoding::Output { .. } => (true, None),
                Decoding::Immutable { .. } => unreachable!("unwrapped immutable decoding"),
            };
            let index = fastctx_hands::lines::Index::open(&read_path, &directory, output, selection, cancel.clone())
                .map_err(failed)?;
            let after = observe(&read_path).map_err(source_failed)?;
            if ((immutable || !output) && snapshot != after)
                || !snapshot.same_file(&after)
                || after.size < snapshot.size
            {
                return Err(failed("the text source changed during indexing"));
            }
            let complete = complete(record_path.as_deref()).map_err(failed)?;
            let extent = Extent { length: index.length, complete, version: 0 };
            let (projection, writes, writes_revision) =
                projection::open(&read_path, snapshot.size, note_path.as_deref(), &cancel).map_err(failed)?;
            projection::verify(projection.as_ref(), expected_annotation.as_deref()).map_err(failed)?;
            Ok::<_, Failure>(State {
                index: Some(index),
                snapshot,
                extent,
                first: 0,
                error: None,
                stale: false,
                unsaid: None,
                projection,
                writes,
                writes_revision,
                notes_broken: false,
            })
        })
        .await
        .map_err(failed)??;
    let source = Arc::new(Source {
        path: path.clone(),
        immutable,
        annotation,
        job_record,
        notes_record,
        state: Mutex::new(opened),
        changed: tokio::sync::watch::channel(0).0,
        closed: tokio::sync::watch::channel(false).0,
        events,
        _watcher: Mutex::new(watcher),
        cancelled: Arc::new(AtomicBool::new(false)),
        requests: Mutex::new(Vec::new()),
    });
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(failed)?;
    let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let key = (from, id.clone());
    let lease = Lease::Here { node: Arc::downgrade(node), key: key.clone(), closed: source.closed.subscribe() };
    node.line_sources.0.lock().expect("line sources").insert(key, source.clone());
    tokio::spawn(monitor(Arc::downgrade(&source), event_rx, source.closed.subscribe()));
    // Catch appends racing the initial bounded snapshot, without waiting for
    // another filesystem event to happen after the monitor starts.
    refresh(source.clone(), Some(initializing)).await?;
    let extent = source.state.lock().expect("line source").extent.clone();
    Ok((
        Reply {
            machine: node.machine(),
            result: Answer::Opened { source: id, path: path.to_string_lossy().into_owned(), extent },
        },
        lease,
    ))
}

/// How often the output of a job still running is looked at again, beside its file's events: its
/// supervisor writes through a file it keeps open, and macOS reports such a file's growth only once
/// it is closed.
const RUNNING_LOOK: std::time::Duration = std::time::Duration::from_secs(1);

/// How soon a source a look failed on for a passing reason is looked at again.
const AGAIN: std::time::Duration = std::time::Duration::from_secs(2);

async fn monitor(
    source: Weak<Source>,
    mut events: tokio::sync::watch::Receiver<std::result::Result<u64, String>>,
    mut closed: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        let running = source.upgrade().is_some_and(|source| {
            source.job_record.is_some() && !source.state.lock().expect("line source").extent.complete
        });
        let stale = source.upgrade().is_some_and(|source| source.state.lock().expect("line source").stale);
        tokio::select! {
            _ = closed.changed() => break,
            event = events.changed() => if event.is_err() { break; },
            () = tokio::time::sleep(RUNNING_LOOK), if running => {}
            () = tokio::time::sleep(AGAIN), if stale => {}
        }
        let Some(source) = source.upgrade() else { break };
        if refresh(source, None).await.is_err() {
            break;
        }
    }
}

async fn refresh(
    source: Arc<Source>,
    initializing: Option<crate::initializing::Guard>,
) -> std::result::Result<(), Failure> {
    tokio::task::spawn_blocking(move || {
        let _initializing = initializing;
        let mut state = source.state.lock().expect("line source");
        if let Some(error) = &state.error {
            return Err(failed(error));
        }
        // An error ends the source where what it shows can no longer be
        // trusted; one that passes, as no descriptor left for a moment,
        // leaves it as it was, to be looked at again.
        let passing = |error: String| (error, true);
        let ending = |error: String| (error, false);
        let update = (|| {
            if let Err(error) = &*source.events.borrow() {
                return Err(ending(error.clone()));
            }
            let snapshot = match observe(&source.path) {
                Ok(snapshot) => snapshot,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(ending(err(error))),
                Err(error) => return Err(passing(err(error))),
            };
            let ended = complete(source.job_record.as_deref()).map_err(passing)?;
            let revision = projection::revision(source.notes_record.as_deref()).map_err(passing)?;
            if state.snapshot == snapshot
                && state.extent.complete == ended
                && state.writes_revision == revision
                && !state.notes_broken
            {
                return Ok(None);
            }
            if source.job_record.is_none()
                || source.immutable && state.snapshot != snapshot
                || !state.snapshot.same_file(&snapshot)
                || snapshot.size < state.snapshot.size
            {
                return Err(ending("the text source was changed, replaced or truncated".into()));
            }
            let grown = match state
                .index
                .as_mut()
                .ok_or_else(|| ending("the source is closed".to_string()))?
                .grow(snapshot.size, source.cancelled.clone())
            {
                Ok(first) => first,
                Err(NotGrown::Passing(error)) => return Err(passing(error)),
                Err(NotGrown::Ended(error)) => return Err(ending(error)),
            };
            let mut first = state.unsaid.map_or(grown, |unsaid| unsaid.min(grown));
            state.unsaid = Some(first);
            match projection::refresh(
                &mut state,
                &source.path,
                snapshot.size,
                source.notes_record.as_deref(),
                revision,
                &source.cancelled,
            ) {
                Ok(changed) => {
                    if changed {
                        first = 0;
                    }
                }
                // The system's own failures pass, as no descriptor or no disk
                // left for a moment; one about what was read ends the source.
                Err(error) if error.raw_os_error().is_some() => return Err(passing(error.to_string())),
                Err(error) => return Err(ending(error.to_string())),
            }
            projection::verify(state.projection.as_ref(), source.annotation.as_deref()).map_err(ending)?;
            state.snapshot = snapshot;
            state.unsaid = None;
            let length = state.index.as_ref().expect("index").length;
            Ok(Some((length, ended, first)))
        })();
        state.stale = matches!(update, Err((_, true)));
        match update {
            Ok(None) | Err((_, true)) => Ok(()),
            Ok(Some((length, complete, first))) => {
                state.extent.length = length;
                state.extent.complete = complete;
                state.extent.version += 1;
                state.first = first;
                source.changed.send_replace(state.extent.version);
                Ok(())
            }
            Err((error, _)) => {
                state.index.take();
                state.projection.take();
                state.error = Some(error.clone());
                source.changed.send_modify(|version| *version += 1);
                Err(failed(error))
            }
        }
    })
    .await
    .map_err(failed)?
}

pub(crate) async fn call(
    node: &Arc<Node>,
    from: MachineName,
    machine: Option<Written>,
    request: Request,
) -> std::result::Result<Reply, Failure> {
    if let Some(machine) = machine.filter(|machine| !node.is_here(machine)) {
        return match node
            .call_with_stream(
                &Acting::Here,
                &machine,
                Purpose::Hands,
                RemoteCall::Lines { request },
                fastctx_net::cancellable::feed,
            )
            .await
            .map_err(failed)??
        {
            RemoteReply::Lines(reply) => Ok(reply),
            _ => Err(failed("the machine answered something other than a line request")),
        };
    }
    let id = match &request {
        Request::Read { source, .. }
        | Request::Find { source, .. }
        | Request::Changed { source, .. }
        | Request::Close { source } => source,
        Request::Open { .. } => return Err(failed("opening a line source requires its owning connection")),
    };
    if matches!(request, Request::Read { .. }) {
        return Err(failed("logical rows require their data connection"));
    }
    let key = (from, id.clone());
    if matches!(request, Request::Close { .. }) {
        close(node, &key);
        return Ok(Reply { machine: node.machine(), result: Answer::Closed });
    }
    let source = node
        .line_sources
        .0
        .lock()
        .expect("line sources")
        .get(&key)
        .cloned()
        .ok_or_else(|| failed("line source is closed or belongs to another caller"))?;
    let mut changes = source.changed.subscribe();
    let mut closed = source.closed.subscribe();
    refresh(source.clone(), None).await?;
    if let Request::Changed { version, .. } = &request {
        loop {
            if *closed.borrow() {
                return Err(failed("line source closed"));
            }
            {
                let state = source.state.lock().expect("line source");
                if let Some(error) = &state.error {
                    return Err(failed(error));
                }
                if state.extent.version > *version {
                    return Ok(Reply {
                        machine: node.machine(),
                        result: Answer::Changed {
                            extent: state.extent.clone(),
                            first_changed_line: if state.extent.version - *version > 1 { 0 } else { state.first },
                        },
                    });
                }
                if state.extent.version < *version {
                    return Err(failed("line version is ahead of the source"));
                }
            }
            tokio::select! { _ = changes.changed() => {}, _ = closed.changed() => return Err(failed("line source closed")) }
        }
    }
    let cancelled = Cancel::default();
    let cancel = cancelled.0.clone();
    {
        let mut requests = source.requests.lock().expect("line requests");
        requests.retain(|request| request.strong_count() > 0);
        requests.push(Arc::downgrade(&cancel));
        if source.cancelled.load(Ordering::Relaxed) {
            return Err(failed("line source closed"));
        }
    }
    let result = tokio::task::spawn_blocking(move || {
        let mut state = source.state.lock().expect("line source");
        let index = state.index.as_mut().ok_or_else(|| "line source closed".to_string())?;
        let result = match request {
            Request::Read { start, count, .. } => {
                Answer::Lines { start, lines: index.read(start, count, &cancel)?, notes: Vec::new() }
            }
            Request::Find { query, position, direction, .. } => {
                Answer::Found { found: index.find(&query, &position, direction, &cancel)? }
            }
            _ => unreachable!("handled before blocking"),
        };
        let snapshot = observe(&source.path).map_err(err)?;
        if source.job_record.is_none() && snapshot != state.snapshot
            || !snapshot.same_file(&state.snapshot)
            || snapshot.size < state.snapshot.size
        {
            return Err("the text source changed during reading".to_string());
        }
        Ok::<_, String>(result)
    })
    .await
    .map_err(failed)?
    .map_err(failed)?;
    Ok(Reply { machine: node.machine(), result })
}

pub(crate) async fn serve(
    node: &Arc<Node>,
    link: &Arc<fastctx_net::link::Link>,
    from: MachineName,
    path: String,
    decoding: Decoding,
    mut stream: fastctx_net::data::Stream,
) -> std::result::Result<RemoteReply, Failure> {
    let opening = open(node, from, None, path, decoding);
    tokio::pin!(opening);
    let (reply, mut lease) = tokio::select! {
        result = &mut opening => result?,
        () = stream.closed() => {
            drop(opening.await);
            return Err(failed("the line source's owner disconnected"));
        },
    };
    stream.say(&link.sign(fastctx_net::sign::Domain::Event, &reply)).await.map_err(failed)?;
    tokio::select! { () = stream.closed() => {}, () = lease.closed() => {} }
    Ok(RemoteReply::Lines(Reply { machine: node.machine(), result: Answer::Closed }))
}

fn close(node: &Node, key: &Key) {
    if let Some(source) = node.line_sources.0.lock().expect("line sources").remove(key) {
        source.closed.send_replace(true);
        source.cancelled.store(true, Ordering::Relaxed);
        for request in source.requests.lock().expect("line requests").iter().filter_map(Weak::upgrade) {
            request.store(true, Ordering::Relaxed);
        }
    }
}
fn observe(path: &std::path::Path) -> std::io::Result<fastctx_platform::file_read::Snapshot> {
    let file = fastctx_platform::file_read::open(path)?;
    fastctx_platform::file_read::snapshot(&file, &file.metadata()?)
}
fn source_failed(error: std::io::Error) -> Failure {
    if error.kind() == std::io::ErrorKind::NotFound {
        Failure::source_missing(error.to_string())
    } else {
        failed(error)
    }
}
fn watch_failed(error: notify::Error, source: &std::path::Path) -> Failure {
    if std::fs::metadata(source).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) {
        Failure::source_missing(error.to_string())
    } else {
        failed(error)
    }
}
fn complete(path: Option<&std::path::Path>) -> std::result::Result<bool, String> {
    let Some(path) = path else { return Ok(true) };
    let record = fastctx_hands::run::record::read(path).map_err(err)?;
    Ok(record.output_ended_at.is_some())
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn failed(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
