//! Watching files and directories for changes without spending one
//! operating-system watcher on each watch.
//!
//! On Linux every `notify` watcher is an inotify instance and a thread, and a
//! user has 128 instances across all their processes by default: a node that
//! watched each conversation's folder with a watcher of its own ran out of
//! them, and what it could no longer watch it could no longer read. Here
//! every watch in the process shares one instance and one thread, and a
//! recursive watch goes only into the directories its watcher keeps
//! (`watch_within`), so a build tree beside what is watched spends no watches.
//! Elsewhere a watcher has a `notify` watcher of its own, as before: Windows
//! and macOS watch a whole tree with one handle.
//!
//! Where the system has no instance, no more watches or no thread to give, a
//! watched place is looked at every two seconds instead, by one thread for
//! the whole process, unless the watcher would rather be told (`polling`).
//!
//! Events are `notify`'s. The shared instance reports changes only, not the
//! openings and closings `notify` reports on Linux.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use notify::{Config, Error, ErrorKind, Event, EventHandler, EventKind, RecursiveMode, Result};

mod poll;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod shared;

/// How often a place no watch could be had for is looked at.
const POLL: Duration = Duration::from_secs(2);

type Handler = Arc<Mutex<Box<dyn EventHandler>>>;

/// Which directories under a recursive watch's root it goes into, by their
/// path relative to that root.
pub type Keep = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// Watches places and hands each change in them to one handler. Dropping it
/// ends every watch it made.
pub struct Watcher {
    handler: Handler,
    follow_links: bool,
    polling: bool,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    shared: Option<shared::Subscription>,
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    own: Option<notify::RecommendedWatcher>,
    poll: Option<poll::Polling>,
}

impl Watcher {
    pub fn new<F: EventHandler>(handler: F) -> Self {
        Self {
            handler: Arc::new(Mutex::new(Box::new(handler))),
            follow_links: true,
            polling: true,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            shared: None,
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            own: None,
            poll: None,
        }
    }

    /// Whether a recursive watch follows links into the directories they
    /// point at. They are followed unless said otherwise.
    pub fn following_links(mut self, follow: bool) -> Self {
        self.follow_links = follow;
        self
    }

    /// Whether a place no watch could be had for is looked at again and
    /// again instead, or the watch fails. It is looked at unless said
    /// otherwise.
    pub fn polling(mut self, polling: bool) -> Self {
        self.polling = polling;
        self
    }

    pub fn watch(&mut self, path: &Path, mode: RecursiveMode) -> Result<()> {
        self.add(path, mode == RecursiveMode::Recursive, None)
    }

    /// Watches the tree at `path`, going only into the directories `keep`
    /// accepts, given their path relative to `path`. Off Linux the whole
    /// tree is watched, at no cost by the directory; the handler still sees
    /// only what it asks about.
    pub fn watch_within(&mut self, path: &Path, keep: impl Fn(&Path) -> bool + Send + Sync + 'static) -> Result<()> {
        self.add(path, true, Some(Arc::new(keep)))
    }

    fn add(&mut self, path: &Path, recursive: bool, keep: Option<Keep>) -> Result<()> {
        let path = std::path::absolute(path).map_err(Error::io)?;
        let root = Root { path, recursive, keep, follow_links: self.follow_links };
        match self.watch_natively(root.clone()) {
            Err(error) if self.polling && exhausted(&error) => self.poll(root),
            watched => watched,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn watch_natively(&mut self, root: Root) -> Result<()> {
        match &mut self.shared {
            Some(subscription) => subscription.watch(root),
            None => {
                let mut subscription = shared::Subscription::new(self.handler.clone())?;
                let watched = subscription.watch(root);
                self.shared = Some(subscription);
                watched
            }
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn watch_natively(&mut self, root: Root) -> Result<()> {
        use notify::Watcher as _;
        let mode = if root.recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        if self.own.is_none() {
            let handler = self.handler.clone();
            let forward =
                move |event| handler.lock().unwrap_or_else(std::sync::PoisonError::into_inner).handle_event(event);
            let config = Config::default().with_follow_symlinks(self.follow_links);
            self.own = Some(notify::RecommendedWatcher::new(forward, config)?);
        }
        let own = self.own.as_mut().expect("just made");
        let watched = own.watch(&root.path, mode);
        if watched.as_ref().is_err_and(exhausted) {
            // A recursive watch stops at the first directory it cannot have:
            // what it had of the tree goes, and polling takes all of it.
            let _ = own.unwatch(&root.path);
        }
        watched
    }

    fn poll(&mut self, root: Root) -> Result<()> {
        if self.poll.is_none() {
            self.poll = Some(poll::Polling::new(self.handler.clone())?);
        }
        self.poll.as_mut().expect("just made").watch(root)
    }
}

/// A watched place.
#[derive(Clone)]
pub(crate) struct Root {
    pub path: PathBuf,
    pub recursive: bool,
    pub keep: Option<Keep>,
    pub follow_links: bool,
}

impl Root {
    /// Whether the directory `path` under this root is gone into.
    fn keeps(&self, path: &Path) -> bool {
        self.recursive
            && path.strip_prefix(&self.path).is_ok_and(|relative| self.keep.as_ref().is_none_or(|keep| keep(relative)))
    }
}

/// Whether a walk goes into the directory `path`: a link only where links
/// are followed, and only once to wherever it leads, so a link back up the
/// tree is not walked round and round.
fn enter(path: &Path, follow_links: bool, followed: &mut HashSet<PathBuf>) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return false };
    if !meta.file_type().is_symlink() {
        return meta.is_dir();
    }
    follow_links && std::fs::canonicalize(path).is_ok_and(|target| target.is_dir() && followed.insert(target))
}

/// The directories directly in `directory` a walk goes into.
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
fn directories(directory: &Path, follow_links: bool, followed: &mut HashSet<PathBuf>) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    entries.flatten().map(|entry| entry.path()).filter(|path| enter(path, follow_links, followed)).collect()
}

/// Whether `error` says the system has no more watches, descriptors or
/// threads to give, rather than that something is wrong with the place.
fn exhausted(error: &Error) -> bool {
    const EAGAIN: i32 = if cfg!(any(target_os = "macos", target_os = "ios")) { 35 } else { 11 };
    const ENFILE: i32 = 23;
    const EMFILE: i32 = 24;
    const ENOSPC: i32 = 28;
    match &error.kind {
        ErrorKind::MaxFilesWatch => true,
        ErrorKind::Io(error) => {
            cfg!(unix) && matches!(error.raw_os_error(), Some(EAGAIN | EMFILE | ENFILE | ENOSPC))
                || error.kind() == io::ErrorKind::OutOfMemory
        }
        _ => false,
    }
}

/// Turns what the system said when asked to watch `path` into `notify`'s
/// words, as `notify` itself does.
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
fn refused(error: io::Error, path: &Path) -> Error {
    const ENOSPC: i32 = 28;
    if error.raw_os_error() == Some(ENOSPC) {
        Error::new(ErrorKind::MaxFilesWatch)
    } else if error.kind() == io::ErrorKind::NotFound {
        Error::path_not_found()
    } else {
        Error::io(error)
    }
    .add_path(path.to_path_buf())
}
