//! Watching files and directories for changes without spending one
//! operating-system watcher on each watch.
//!
//! On Linux every `notify` watcher is an inotify instance and a thread, and a
//! user has 128 instances across all their processes by default: a node that
//! watched each conversation's folder with a watcher of its own ran out of
//! them, and what it could no longer watch it could no longer read. Here every
//! non-recursive watch in the process shares one instance and one thread.
//! Where Linux gives no instance or no more watches, a watched place is looked
//! at every two seconds instead, unless the watcher would rather be told
//! (`polling(false)`). A recursive watch, and every watch off Linux, has a
//! `notify` watcher of its own, as before.
//!
//! Events are `notify`'s. The shared instance reports changes only, not the
//! openings and closings `notify` reports on Linux.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

pub use notify::{Config, Error, ErrorKind, Event, EventHandler, EventKind, RecursiveMode, Result};

#[cfg(any(target_os = "linux", target_os = "android"))]
mod shared;

/// How often a place no watch could be had for is looked at.
const POLL: Duration = Duration::from_secs(2);

type Handler = Arc<Mutex<Box<dyn EventHandler>>>;

/// Watches places and hands each change in them to one handler. Dropping it
/// ends every watch it made.
pub struct Watcher {
    handler: Handler,
    follow_links: bool,
    polling: bool,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    shared: Option<shared::Subscription>,
    own: Option<notify::RecommendedWatcher>,
    poll: Option<notify::PollWatcher>,
}

impl Watcher {
    pub fn new<F: EventHandler>(handler: F) -> Self {
        Self {
            handler: Arc::new(Mutex::new(Box::new(handler))),
            follow_links: true,
            polling: true,
            #[cfg(any(target_os = "linux", target_os = "android"))]
            shared: None,
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
        let path = std::path::absolute(path).map_err(Error::io)?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if mode == RecursiveMode::NonRecursive {
            let watched = match &mut self.shared {
                Some(subscription) => subscription.watch(&path),
                None => shared::Subscription::new(self.handler.clone()).and_then(|mut subscription| {
                    let watched = subscription.watch(&path);
                    self.shared = Some(subscription);
                    watched
                }),
            };
            return match watched {
                Err(error) if self.polling && exhausted(&error) => self.poll(&path, mode),
                watched => watched,
            };
        }
        let watched = match &mut self.own {
            Some(own) => notify::Watcher::watch(own, &path, mode),
            None => {
                let config = Config::default().with_follow_symlinks(self.follow_links);
                <notify::RecommendedWatcher as notify::Watcher>::new(forward(&self.handler), config).and_then(
                    |mut own| {
                        let watched = notify::Watcher::watch(&mut own, &path, mode);
                        self.own = Some(own);
                        watched
                    },
                )
            }
        };
        match watched {
            Err(error) if self.polling && exhausted(&error) => {
                // A recursive watch stops at the first directory it cannot
                // have: what it had of the tree goes, and polling takes all.
                if let Some(own) = &mut self.own {
                    let _ = notify::Watcher::unwatch(own, &path);
                }
                self.poll(&path, mode)
            }
            watched => watched,
        }
    }

    fn poll(&mut self, path: &Path, mode: RecursiveMode) -> Result<()> {
        if self.poll.is_none() {
            let config = Config::default().with_poll_interval(POLL).with_follow_symlinks(self.follow_links);
            self.poll = Some(notify::PollWatcher::new(forward(&self.handler), config)?);
        }
        notify::Watcher::watch(self.poll.as_mut().expect("just made"), path, mode)
    }
}

/// The handler as each kind of watch calls it, one call at a time.
fn forward(handler: &Handler) -> impl FnMut(Result<Event>) + Send + 'static {
    let handler = handler.clone();
    move |event| handler.lock().unwrap_or_else(PoisonError::into_inner).handle_event(event)
}

/// Whether `error` says the system has no more watches or descriptors to
/// give, rather than that something is wrong with the place.
fn exhausted(error: &Error) -> bool {
    const ENFILE: i32 = 23;
    const EMFILE: i32 = 24;
    const ENOSPC: i32 = 28;
    match &error.kind {
        ErrorKind::MaxFilesWatch => true,
        ErrorKind::Io(error) => matches!(error.raw_os_error(), Some(EMFILE | ENFILE | ENOSPC)),
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
