//! One watcher and one fair queue for all local Claude sources. Changes coalesce
//! by source path; successful idle sources perform no timed content polling.
use super::{
    claim::Claim,
    engine::{self, Outcome},
};
use crate::Node;
use fastctx_hosts::claude::exchange;
use fastctx_types::ResidentId;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::Notify;

#[derive(Clone)]
struct Entry {
    resident: ResidentId,
    session: String,
    claim: Option<Claim>,
    retry: Option<Instant>,
    problem: Option<String>,
}
#[derive(Default)]
struct State {
    entries: HashMap<PathBuf, Entry>,
    queue: VecDeque<PathBuf>,
    queued: HashSet<PathBuf>,
    retries: BTreeMap<(Instant, PathBuf), ()>,
}
impl State {
    fn enqueue(&mut self, path: PathBuf) {
        if !self.entries.contains_key(&path) {
            return;
        }
        if let Some(old) = self.entries.get_mut(&path).and_then(|e| e.retry.take()) {
            self.retries.remove(&(old, path.clone()));
        }
        if self.queued.insert(path.clone()) {
            self.queue.push_back(path);
        }
    }
    fn next(&mut self) -> Option<(PathBuf, Entry)> {
        while let Some((when, path)) = self.retries.first_key_value().map(|(key, _)| key.clone()) {
            if when > Instant::now() {
                break;
            }
            self.retries.remove(&(when, path.clone()));
            self.enqueue(path);
        }
        while let Some(path) = self.queue.pop_front() {
            self.queued.remove(&path);
            if let Some(entry) = self.entries.get(&path) {
                return Some((path, entry.clone()));
            }
        }
        None
    }
    fn retry(&mut self, path: &Path, delay: Duration) {
        let Some(entry) = self.entries.get_mut(path) else { return };
        if let Some(old) = entry.retry.take() {
            self.retries.remove(&(old, path.to_path_buf()));
        }
        let when = Instant::now() + delay;
        entry.retry = Some(when);
        self.retries.insert((when, path.to_path_buf()), ());
    }
}
struct Shared {
    state: Mutex<State>,
    changed: Notify,
}
pub(crate) struct Service {
    shared: Arc<Shared>,
    watcher: Mutex<fastctx_watch::Watcher>,
    directories: Mutex<HashSet<PathBuf>>,
}
pub(crate) fn start(node: &Arc<Node>) -> io::Result<()> {
    if node.ingestion.get().is_some() {
        return Ok(());
    }
    let shared = Arc::new(Shared { state: Mutex::new(State::default()), changed: Notify::new() });
    let weak = Arc::downgrade(&shared);
    let watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        if !crate::file_events::may_change(&event) {
            return;
        }
        let Some(shared) = weak.upgrade() else { return };
        let mut state = shared.state.lock().expect("never poisoned");
        let paths = match event {
            Ok(event) if !event.need_rescan() => event.paths,
            _ => state.entries.keys().cloned().collect(),
        };
        for path in paths {
            state.enqueue(path);
        }
        drop(state);
        shared.changed.notify_one();
    });
    let service = Arc::new(Service {
        shared: shared.clone(),
        watcher: Mutex::new(watcher),
        directories: Mutex::new(HashSet::new()),
    });
    if node.ingestion.set(service).is_err() {
        return Ok(());
    }
    let weak = Arc::downgrade(node);
    fastctx_platform::task::spawn(async move { run(weak, shared).await });
    Ok(())
}
pub(crate) fn register(node: &Arc<Node>, resident: &ResidentId, directory: &Path, session: &str) -> io::Result<()> {
    if node.foundation.as_ref().and_then(|f| f.hosted(node, resident)).is_none() {
        return Ok(());
    }
    let Some(service) = node.ingestion.get() else { return Ok(()) };
    let Some(path) = exchange::transcript(directory)? else { return Ok(()) };
    let folder = path.parent().ok_or_else(|| io::Error::other("native source has no parent"))?;
    if !fastctx_platform::disk::exists(folder)? {
        return Ok(());
    }
    let named = crate::file_events::named(folder)?
        .join(path.file_name().ok_or_else(|| io::Error::other("source has no filename"))?);
    {
        let mut directories = service.directories.lock().expect("never poisoned");
        if !directories.contains(folder) {
            service
                .watcher
                .lock()
                .expect("never poisoned")
                .watch(folder, notify::RecursiveMode::NonRecursive)
                .map_err(io::Error::other)?;
            directories.insert(folder.to_path_buf());
        }
    }
    let mut state = service.shared.state.lock().expect("never poisoned");
    state.entries.entry(named.clone()).or_insert(Entry {
        resident: resident.clone(),
        session: session.into(),
        claim: None,
        retry: None,
        problem: None,
    });
    state.enqueue(named);
    drop(state);
    service.shared.changed.notify_one();
    Ok(())
}
async fn run(node: Weak<Node>, shared: Arc<Shared>) {
    let observer = fastctx_sync::metrics::observe::current();
    loop {
        let next = { shared.state.lock().expect("never poisoned").next() };
        let Some((path, mut entry)) = next else {
            let until =
                { shared.state.lock().expect("never poisoned").retries.first_key_value().map(|((when, _), _)| *when) };
            match until {
                Some(until) => {
                    tokio::select! {_=shared.changed.notified()=>{},_=tokio::time::sleep_until(until.into())=>{}}
                }
                None => shared.changed.notified().await,
            }
            continue;
        };
        let Some(node) = node.upgrade() else { return };
        let allowed = node
            .residents
            .about(&entry.resident)
            .is_some_and(|(about, _)| about.world_managed() && crate::logins::workbench::reads_host(&node, &about))
            && node.residents.deleted_session(&entry.session).is_none();
        if !allowed {
            shared.state.lock().expect("never poisoned").entries.remove(&path);
            continue;
        }
        let result = async {
            if entry.claim.is_none() {
                entry.claim = Claim::open(&node, &entry.resident, &entry.session).await?;
            }
            match &entry.claim {
                Some(claim) => Box::pin(engine::advance(&node, claim, &path, observer)).await.map(Some),
                None => Ok(None),
            }
        }
        .await;
        {
            let mut state = shared.state.lock().expect("never poisoned");
            match result {
                Ok(Some(outcome)) => {
                    entry.problem = None;
                    state.entries.insert(path.clone(), entry);
                    if matches!(outcome, Outcome::More) {
                        state.enqueue(path);
                    }
                }
                Ok(None) => {
                    state.entries.remove(&path);
                }
                Err(error) => {
                    let problem = error.to_string();
                    if entry.problem.as_ref() != Some(&problem) {
                        node.log.line(format_args!("ingesting Claude source {}: {problem}", path.display()));
                    }
                    entry.problem = Some(problem);
                    state.entries.insert(path.clone(), entry);
                    state.retry(
                        &path,
                        if matches!(error, fastctx_sync::store::Error::Busy) {
                            Duration::from_millis(500)
                        } else {
                            Duration::from_secs(30)
                        },
                    );
                }
            }
        }
        drop(node);
        tokio::task::yield_now().await;
    }
}
