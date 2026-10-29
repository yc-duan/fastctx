//! A picker owns a Search: one background scan, a live file-name index and
//! filesystem watches. Dropping it cancels the scan and releases the watches.
//! Hidden files participate; repository ignore rules apply. Descendant .git,
//! node_modules and target directories, links and special files never do.
//! Explicit roots may name a directory or one regular file.

mod index;
mod paths;
mod rank;
mod rules;

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};

use fastctx_types::file_search::FileSearchResult;
use notify::{EventKind, RecursiveMode};

#[derive(Default)]
struct State {
    revision: u64,
    policy_revision: u64,
    policy_dirty: bool,
    dirty: bool,
    index: Option<Arc<index::Index>>,
    error: Option<String>,
    watch_error: Option<String>,
}

struct Shared {
    roots: Vec<PathBuf>,
    closed: AtomicBool,
    state: Mutex<State>,
    wake: mpsc::SyncSender<()>,
}

impl Shared {
    fn request_refresh(&self) {
        let mut state = self.state.lock().expect("search state poisoned");
        if state.dirty {
            return;
        }
        state.revision += 1;
        state.dirty = true;
        drop(state);
        let _ = self.wake.try_send(());
    }

    fn invalidate(&self, policy_changed: bool) {
        let mut state = self.state.lock().expect("search state poisoned");
        state.revision += 1;
        state.dirty = true;
        if policy_changed {
            state.policy_revision += 1;
            state.policy_dirty = true;
        }
        drop(state);
        // The queued unit means "look at the latest state", not one event.
        // Coalescing it loses no change and cannot grow with a build's output.
        let _ = self.wake.try_send(());
    }

    fn event(&self, event: notify::Result<notify::Event>) {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                self.state.lock().expect("search state poisoned").watch_error = Some(error.to_string());
                self.invalidate(true);
                return;
            }
        };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let state = self.state.lock().expect("search state poisoned");
        // Windows reports content and attribute writes as Modify(Any).
        // Only name changes alter this index, except changes to its policy
        // files or changes that can repair a previous scan failure.
        let names_changed =
            !matches!(event.kind, EventKind::Modify(kind) if !matches!(kind, notify::event::ModifyKind::Name(_)));
        let policy_changed = event.need_rescan()
            || event.paths.iter().any(|path| {
                if let Some(index) = &state.index
                    && (index
                        .rules
                        .get(path)
                        .or_else(|| index.outer_rules.get(path))
                        .is_some_and(|stamp| names_changed || !stamp.is_directory())
                        || (names_changed && index.outer_rules.keys().any(|file| file.starts_with(path))))
                {
                    return true;
                }
                self.roots.iter().any(|root| {
                    (names_changed && root.starts_with(path))
                        || (path.starts_with(root)
                            && matches!(
                                path.file_name().and_then(|name| name.to_str()),
                                Some(".gitignore" | ".ignore")
                            ))
                })
            });
        let relevant = policy_changed
            || event.paths.iter().any(|path| {
                self.roots.iter().any(|root| {
                    if root.starts_with(path) {
                        return names_changed || state.error.is_some() || state.watch_error.is_some();
                    }
                    if !path.starts_with(root) || paths::excluded(path, root, false) {
                        return false;
                    }
                    if state.error.is_some() || state.watch_error.is_some() {
                        return true;
                    }
                    if matches!(path.file_name().and_then(|name| name.to_str()), Some(".gitignore" | ".ignore")) {
                        return true;
                    }
                    if !names_changed {
                        return false;
                    }
                    state
                        .index
                        .as_ref()
                        .is_none_or(|index| path.parent().is_some_and(|parent| index.directories.contains(parent)))
                })
            });
        drop(state);
        if relevant {
            self.invalidate(policy_changed);
        }
    }
}

pub struct Search {
    shared: Arc<Shared>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Search {
    /// Validate absolute roots and start indexing. No directory enumeration
    /// occurs on this thread. Empty roots produce an empty, ready index.
    pub fn new(roots: Vec<String>) -> io::Result<Self> {
        let roots = paths::roots(roots)?;
        let (wake, receiver) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            roots,
            closed: AtomicBool::new(false),
            state: Mutex::new(State { dirty: true, ..State::default() }),
            wake,
        });
        let work = shared.clone();
        let worker = std::thread::Builder::new().name("file-search-index".into()).spawn(move || {
            if let Err(error) = watch_and_index(&work, receiver) {
                work.state.lock().expect("search state poisoned").error = Some(error.to_string());
            }
        })?;
        Ok(Self { shared, worker: Some(worker) })
    }

    /// Stop scanning and matching, even while queries still hold this search.
    /// Dropping the last owner joins the scanner and releases its watches.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::Release);
        let _ = self.shared.wake.try_send(());
    }

    /// Rank without rescanning directories. A refresh can serve its prior
    /// snapshot, with each returned path and policy rechecked. Filesystem errors are errors,
    /// including unreadable or malformed ignore rules, never empty success.
    pub fn query(&self, query: String, limit: usize) -> io::Result<FileSearchResult> {
        check_open(&self.shared.closed)?;
        let pending = || FileSearchResult::Matches { query: query.clone(), ready: false, total: 0, files: vec![] };
        let (index, revision, complete) = {
            let state = self.shared.state.lock().expect("search state poisoned");
            if let Some(error) = state.watch_error.as_ref().or(state.error.as_ref()) {
                let error = io::Error::other(error.clone());
                drop(state);
                self.shared.request_refresh();
                return Err(error);
            }
            if state.policy_dirty {
                return Ok(pending());
            }
            let Some(index) = state.index.clone() else {
                return Ok(pending());
            };
            (index, state.revision, !state.dirty)
        };
        let Some((total, files)) = rank::query(&index, &query, limit, &self.shared.closed)? else {
            self.shared.request_refresh();
            return Ok(pending());
        };
        let state = self.shared.state.lock().expect("search state poisoned");
        if let Some(error) = state.watch_error.as_ref().or(state.error.as_ref()) {
            return Err(io::Error::other(error.clone()));
        }
        if state.policy_dirty {
            return Ok(pending());
        }
        check_open(&self.shared.closed)?;
        let ready = complete && !state.dirty && state.revision == revision;
        Ok(FileSearchResult::Matches { query, ready, total, files })
    }
}

fn check_open(closed: &AtomicBool) -> io::Result<()> {
    if closed.load(Ordering::Acquire) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "file search closed"))
    } else {
        Ok(())
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        self.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn watch_and_index(shared: &Arc<Shared>, receiver: mpsc::Receiver<()>) -> io::Result<()> {
    let mut watcher = None;
    loop {
        if shared.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let (revision, policy_revision) = {
            let state = shared.state.lock().expect("search state poisoned");
            (state.revision, state.policy_revision)
        };
        let setup = new_watcher(shared).and_then(|fresh| {
            // Keep the old watches until their replacement is installed.
            // A removed directory loses its OS watch even if its path later
            // reappears, so registrations are rebuilt with every scan.
            let mut fresh = Watches { watcher: fresh, paths: HashSet::new(), recursive: Vec::new() };
            let result = fresh.roots(&shared.roots);
            watcher = Some(Arc::new(Mutex::new(fresh)));
            result
        });
        let built = setup.and_then(|()| {
            let watching = watcher.as_ref().expect("watcher installed").clone();
            let index = index::Index::build(shared.roots.clone(), &shared.closed, move |path| {
                watching.lock().expect("watches poisoned").directory(path)
            })?;
            for file in index.rules.keys().chain(index.outer_rules.keys()) {
                if let Some(parent) = file.parent()
                    && !index.roots.iter().any(|root| index.directories.contains(root) && parent.starts_with(root))
                {
                    watcher.as_ref().expect("watcher installed").lock().expect("watches poisoned").directory(parent)?;
                }
            }
            if rules::changed(&index.rules)? || rules::changed(&index.outer_rules)? {
                Ok(None)
            } else {
                Ok(Some(index))
            }
        });
        if shared.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut state = shared.state.lock().expect("search state poisoned");
        if state.policy_revision != policy_revision {
            continue;
        }
        let changed = state.revision != revision;
        let mut replaced = None;
        match built {
            Ok(Some(index)) => {
                replaced = state.index.replace(Arc::new(index));
                state.error = None;
                state.watch_error = None;
                state.policy_dirty = false;
                state.dirty = changed;
            }
            Ok(None) => continue,
            Err(error) => {
                if changed {
                    continue;
                }
                state.error = Some(error.to_string());
                state.dirty = false;
            }
        }
        drop(state);
        // Freeing a large index must not hold up snapshot readers.
        drop(replaced);
        if changed {
            continue;
        }
        while receiver.try_recv().is_ok() {}
        if shared.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        if shared.state.lock().expect("search state poisoned").revision == revision {
            receiver.recv().map_err(io::Error::other)?;
        }
    }
}

/// A watcher for this search's places. One the system has no watch for is
/// a failed scan, said to whoever searches, not a place looked at again and
/// again: a tree is too many places for that.
fn new_watcher(shared: &Arc<Shared>) -> io::Result<fastctx_watch::Watcher> {
    let weak: Weak<Shared> = Arc::downgrade(shared);
    Ok(fastctx_watch::Watcher::new(move |event| {
        if let Some(shared) = weak.upgrade() {
            shared.event(event);
        }
    })
    .following_links(false)
    .polling(false))
}

struct Watches {
    watcher: fastctx_watch::Watcher,
    paths: HashSet<PathBuf>,
    recursive: Vec<PathBuf>,
}

impl Watches {
    fn roots(&mut self, roots: &[PathBuf]) -> io::Result<()> {
        for root in roots {
            for parent in root.parent().into_iter().flat_map(Path::ancestors) {
                self.directory(parent)?;
            }
            if let Ok(metadata) = std::fs::symlink_metadata(root) {
                if paths::linked(&metadata) {
                    return Err(io::Error::other("search root was replaced with a link"));
                }
                let directory = if metadata.is_dir() { root.as_path() } else { root.parent().expect("absolute file") };
                if cfg!(windows) && metadata.is_dir() {
                    // Windows watches a subtree with one native handle. On
                    // Unix, recursive notify watches enumerate it themselves,
                    // including ignored directories; install those watches
                    // only as our filtered walker admits each directory.
                    self.watcher.watch(directory, RecursiveMode::Recursive).map_err(io::Error::other)?;
                    self.paths.insert(directory.to_owned());
                    self.recursive.push(directory.to_owned());
                } else {
                    self.directory(directory)?;
                }
            }
        }
        Ok(())
    }

    fn directory(&mut self, directory: &Path) -> io::Result<()> {
        // Missing ignore files still have a watched existing ancestor, so their
        // creation is observed. The recursive root watch covers inner folders.
        let parent = directory
            .ancestors()
            .find(|path| path.is_dir())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no parent directory to watch"))?;
        if self.recursive.iter().any(|root| parent.starts_with(root)) {
            return Ok(());
        }
        if self.paths.insert(parent.to_owned()) {
            self.watcher.watch(parent, RecursiveMode::NonRecursive).map_err(io::Error::other)?;
        }
        Ok(())
    }
}
