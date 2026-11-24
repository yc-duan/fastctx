//! User-only line sources, owned by live connections and the holding file.

mod copied;
pub(crate) mod drill;
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
    /// What the state shows, published under its lock as it changes.
    shown: tokio::sync::watch::Sender<Shown>,
    /// Told as a look fails for a passing reason, so that the watch looks
    /// again soon whoever's look it was.
    again: Arc<tokio::sync::Notify>,
    closed: tokio::sync::watch::Sender<bool>,
    events: tokio::sync::watch::Sender<std::result::Result<u64, String>>,
    _watcher: Mutex<fastctx_watch::Watcher>,
    cancelled: Arc<AtomicBool>,
    requests: Mutex<Vec<Weak<AtomicBool>>>,
    /// Whether a job's output written out from the copy here is whole, as
    /// the copy said when it was written out; none for any other source.
    copy_whole: Option<bool>,
    /// A job's output written out from the copy here (`copied`), gone with
    /// the source: last, so what reads it is let go first.
    _copied: Option<crate::attachments::kept_here::Passing>,
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
/// What a source shows as its state last settled, kept apart from that
/// state: a reader holds the state for as long as its lines are taken, and
/// what waits on a source or asks whether to look at it again reads this
/// instead, on whatever thread it runs (a link's own must never wait
/// behind a reader).
#[derive(Clone)]
struct Shown {
    extent: Extent,
    first: u64,
    error: Option<String>,
    stale: bool,
}
impl Shown {
    fn of(state: &State) -> Self {
        Self { extent: state.extent.clone(), first: state.first, error: state.error.clone(), stale: state.stale }
    }
}
/// Publishes what `state` shows, under its lock: telling whoever waits on
/// the source as `tell` says, which whether to look again alone does not.
fn show(source: &Source, state: &State, tell: bool) {
    source.shown.send_if_modified(|shown| {
        *shown = Shown::of(state);
        tell
    });
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
        // A job's output the copy here has whole, or what came of it while
        // its machine is away, is read here (plan 3-5); as history read it
        // (`Immutable`), only whole.
        if let Some((job, immutable)) = output_job(&decoding)
            && let Some((copy, whole)) = copied::output(node, &machine, job, &path).await
            && (whole || !immutable)
        {
            let local = copy.path().to_string_lossy().into_owned();
            return opened_here(node, from, Some((machine.name.clone(), path)), local, decoding, Some((copy, whole))).await;
        }
        let (link, account) = node.speaking(&Acting::Here, &machine, Purpose::Hands).await.map_err(failed)?;
        let (reply, lease) = fastctx_net::lines::open(link, machine.name, account, path, decoding).await?;
        return Ok((reply, Lease::Remote(lease)));
    }
    // A job run here whose files went as their retention says, its output
    // whole in its conversation's scope here (`job_assets::whole_here`), is
    // read from there as a copy of another machine's job is. Its notes went
    // with its files: history that read some of them is told they changed.
    if let Some((job, immutable)) = kept_output_job(&decoding)
        && gone(&path)
    {
        let here = Written { name: node.machine(), hub: None };
        if let Some((copy, whole)) = copied::output(node, &here, job, &path).await
            && (whole || !immutable)
        {
            let local = copy.path().to_string_lossy().into_owned();
            return opened_here(node, from, Some((here.name, path)), local, decoding, Some((copy, whole))).await;
        }
    }
    opened_here(node, from, None, path, decoding, None).await
}

/// The job whose output `decoding` reads, and whether history read it so
/// (`Immutable`), which takes it only whole. History's notes on it are kept
/// beside its files alone: one read with them is read there (codex-847f57fd
/// #6).
fn output_job(decoding: &Decoding) -> Option<(&fastctx_types::JobId, bool)> {
    match decoding {
        Decoding::Output { job } => Some((job, false)),
        Decoding::Immutable { decoding: inner, annotation, .. } if annotation.as_deref().is_none_or(str::is_empty) => {
            match inner.as_ref() {
                Decoding::Output { job } => Some((job, true)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The job whose output `decoding` reads, and whether history read it so
/// (`Immutable`), which takes it only whole, whatever notes it read with it.
fn kept_output_job(decoding: &Decoding) -> Option<(&fastctx_types::JobId, bool)> {
    match decoding {
        Decoding::Output { job } => Some((job, false)),
        Decoding::Immutable { decoding: inner, .. } => match inner.as_ref() {
            Decoding::Output { job } => Some((job, true)),
            _ => None,
        },
        _ => None,
    }
}

/// Whether nothing is at `path` on this machine any more.
fn gone(path: &str) -> bool {
    fastctx_platform::local_path(path).is_ok_and(|path| {
        matches!(fastctx_platform::disk::metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    })
}

/// Whether the line source `source` of `from` was opened on this machine: a
/// job's output read from the copy here goes on being read here, though the
/// requests name the machine it ran on.
fn held_here(node: &Node, from: &MachineName, source: &str) -> bool {
    node.line_sources.0.lock().expect("line sources").contains_key(&(from.clone(), source.to_string()))
}

/// Opens the line source at `path` on this machine: a file of its own, or a
/// job's output written out from the copy here (`copy`) for the machine it
/// ran on, at the path it has there (`standing_for`).
async fn opened_here(
    node: &Arc<Node>,
    from: MachineName,
    standing_for: Option<(MachineName, String)>,
    path: String,
    decoding: Decoding,
    copy: Option<(crate::attachments::kept_here::Passing, bool)>,
) -> std::result::Result<(Reply, Lease), Failure> {
    let (copy, copy_whole) = match copy {
        Some((copy, whole)) => (Some(copy), Some(whole)),
        None => (None, None),
    };
    let initializing = crate::initializing::take().await?;
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
    if copy.is_none() {
        crate::replicas::sync::repair_paths(node, std::slice::from_ref(&path)).await.map_err(failed)?;
    }
    let (job_record, notes_record) = match &decoding {
        // Written out from the copy here: no record of the job on this
        // machine, and the file grows no more.
        Decoding::Output { .. } if copy.is_some() => (None, None),
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
            let complete = match copy_whole {
                Some(whole) => whole,
                None => complete(record_path.as_deref()).map_err(failed)?,
            };
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
    let shown = Shown::of(&opened);
    let source = Arc::new(Source {
        path: path.clone(),
        immutable,
        annotation,
        job_record,
        notes_record,
        state: Mutex::new(opened),
        shown: tokio::sync::watch::channel(shown).0,
        again: Arc::default(),
        closed: tokio::sync::watch::channel(false).0,
        events,
        _watcher: Mutex::new(watcher),
        cancelled: Arc::new(AtomicBool::new(false)),
        requests: Mutex::new(Vec::new()),
        copy_whole,
        _copied: copy,
    });
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(failed)?;
    let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let key = (from, id.clone());
    let lease = Lease::Here { node: Arc::downgrade(node), key: key.clone(), closed: source.closed.subscribe() };
    let watching = source.closed.subscribe();
    node.line_sources.0.lock().expect("line sources").insert(key, source.clone());
    fastctx_platform::task::spawn(monitor(Arc::downgrade(&source), event_rx, watching, source.again.clone()));
    // Catch appends racing the initial bounded snapshot, without waiting for
    // another filesystem event to happen after the monitor starts.
    refresh(source.clone(), Some(initializing)).await?;
    let extent = source.shown.borrow().extent.clone();
    let (machine, path) = standing_for.unwrap_or_else(|| (node.machine(), path.to_string_lossy().into_owned()));
    Ok((Reply { machine, result: Answer::Opened { source: id, path, extent } }, lease))
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
    again: Arc<tokio::sync::Notify>,
) {
    loop {
        if *closed.borrow() {
            break;
        }
        // As last shown: a reader may hold the state as long as it takes.
        let (running, stale) = source.upgrade().map_or((false, false), |source| {
            let shown = source.shown.borrow();
            (source.job_record.is_some() && !shown.extent.complete, shown.stale)
        });
        tokio::select! {
            _ = closed.changed() => break,
            event = events.changed() => if event.is_err() { break; },
            () = tokio::time::sleep(RUNNING_LOOK), if running => {}
            () = tokio::time::sleep(AGAIN), if stale => {}
            // A look failed for a passing reason, maybe another's: looked at
            // again once `AGAIN` has passed.
            () = again.notified() => continue,
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
    fastctx_platform::task::spawn_blocking(move || {
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
            let ended = match source.copy_whole {
                Some(whole) => whole,
                None => complete(source.job_record.as_deref()).map_err(passing)?,
            };
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
            Ok(None) | Err((_, true)) => {
                show(&source, &state, false);
                // Shown first: the watch, told, reads it to wait to look again.
                if state.stale {
                    source.again.notify_one();
                }
                Ok(())
            }
            Ok(Some((length, complete, first))) => {
                state.extent.length = length;
                state.extent.complete = complete;
                state.extent.version += 1;
                state.first = first;
                show(&source, &state, true);
                Ok(())
            }
            Err((error, _)) => {
                state.index.take();
                state.projection.take();
                state.error = Some(error.clone());
                show(&source, &state, true);
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
    let named = match &request {
        Request::Read { source, .. }
        | Request::Find { source, .. }
        | Request::Changed { source, .. }
        | Request::Close { source } => Some(source.as_str()),
        Request::Open { .. } => None,
    };
    if let Some(machine) = machine.filter(|machine| !node.is_here(machine))
        && !named.is_some_and(|source| held_here(node, &from, source))
    {
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
    if let Request::Changed { version, .. } = &request {
        let look = async {
            crate::replicas::sync::repair_paths(node, std::slice::from_ref(&source.path)).await.map_err(failed)?;
            refresh(source.clone(), None).await
        };
        let result = changed(&source, *version, look).await?;
        return Ok(Reply { machine: node.machine(), result });
    }
    crate::replicas::sync::repair_paths(node, std::slice::from_ref(&source.path)).await.map_err(failed)?;
    refresh(source.clone(), None).await?;
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
    let result = fastctx_platform::task::spawn_blocking(move || {
        let (mut state, result) = match request {
            Request::Read { start, count, .. } => {
                let mut state = source.state.lock().expect("line source");
                let index = state.index.as_mut().ok_or_else(|| "line source closed".to_string())?;
                let lines = index.read(start, count, &cancel)?;
                (state, Answer::Lines { start, lines, notes: Vec::new() })
            }
            Request::Find { query, position, direction, .. } => {
                let (state, found) = find(&source, &query, &position, direction, &cancel)?;
                (state, Answer::Found { found })
            }
            _ => unreachable!("handled before blocking"),
        };
        let state = &mut *state;
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

/// Waits for `source` to show a change after `version`, telling what it
/// shows last: a change shown already is told at once, as a reader may hold
/// the source as long as its lines take, whatever the file has become since
/// (a read looks at the file itself). While it shows none the source is
/// looked at afresh (`look`), once; a change shown or the source closed
/// meanwhile is told without waiting for that look.
async fn changed(
    source: &Source,
    version: u64,
    look: impl Future<Output = std::result::Result<(), Failure>>,
) -> std::result::Result<Answer, Failure> {
    let mut shown = source.shown.subscribe();
    let mut closed = source.closed.subscribe();
    let mut look = std::pin::pin!(look);
    let mut looking = true;
    loop {
        if *closed.borrow() {
            return Err(failed("line source closed"));
        }
        if let Some(result) = since(&mut shown, version) {
            return result;
        }
        tokio::select! {
            looked = &mut look, if looking => {
                looked?;
                looking = false;
            }
            _ = shown.changed() => {}
            _ = closed.changed() => return Err(failed("line source closed")),
        }
    }
}

/// What a change after `version` says, as the source shows it now: nothing
/// while it shows that version still. What it shows is seen, so that waiting
/// after wakes only as it shows more.
fn since(
    shown: &mut tokio::sync::watch::Receiver<Shown>,
    version: u64,
) -> Option<std::result::Result<Answer, Failure>> {
    let shown = shown.borrow_and_update().clone();
    if let Some(error) = shown.error {
        return Some(Err(failed(error)));
    }
    if shown.extent.version > version {
        let first_changed_line = if shown.extent.version - version > 1 { 0 } else { shown.first };
        return Some(Ok(Answer::Changed { extent: shown.extent, first_changed_line }));
    }
    if shown.extent.version < version {
        return Some(Err(failed("line version is ahead of the source")));
    }
    None
}

/// How many bytes of lines one stretch of a search reads holding the
/// source: between stretches it grows, and is read, as it would without one.
const SEARCH_STRETCH: u64 = 1 << 20;

/// How many times a search starts over because lines it had searched
/// changed before it gives up.
const SEARCH_RESTARTS: usize = 8;

/// Finds `query` from `position` a stretch at a time, letting the source go
/// between stretches. Lines that grew past where the search reached are
/// searched as they are now; a change to a line it had searched starts it
/// over, so it finds what one search holding the source throughout would.
/// Gives back the source held after the last stretch, for the caller's last
/// look at the file.
fn find<'a>(
    source: &'a Source,
    query: &str,
    position: &fastctx_types::lines::Position,
    direction: fastctx_types::lines::Direction,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(std::sync::MutexGuard<'a, State>, Option<fastctx_types::lines::Match>), String> {
    use fastctx_hands::lines::Searched;
    use fastctx_types::lines::{Direction, Position};
    let mut at = position.clone();
    let mut version = None;
    let mut restarts = 0;
    loop {
        let mut state = source.state.lock().expect("line source");
        let now = state.extent.version;
        if let Some(seen) = version
            && seen != now
        {
            // The first line the change touched; any, when several came.
            let first = if now == seen + 1 { state.first } else { 0 };
            let searched = match direction {
                Direction::Forward => first < at.line,
                Direction::Backward => first <= position.line,
            };
            if searched {
                restarts += 1;
                if restarts > SEARCH_RESTARTS {
                    return Err("the text kept changing where it was being searched".into());
                }
                at = position.clone();
            }
        }
        version = Some(now);
        let index = state.index.as_mut().ok_or_else(|| "line source closed".to_string())?;
        match index.find_within(query, &at, position, direction, cancel, SEARCH_STRETCH)? {
            Searched::Done(found) => return Ok((state, found)),
            Searched::From(line) => at = Position { line, offset: None },
        }
    }
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
    // Out of the table before it is told: a source let go of for the last
    // time ends its watch and what was written out for it, waiting on
    // neither the table nor its readers.
    let removed = node.line_sources.0.lock().expect("line sources").remove(key);
    if let Some(source) = removed {
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
    if fastctx_platform::disk::metadata(source).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) {
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
