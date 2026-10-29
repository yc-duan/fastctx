//! One inotify instance and one thread for every non-recursive watch in the
//! process.
//!
//! inotify gives one watch per inode in an instance, whoever asks: a place two
//! watchers watch is one watch, kept while either of them still wants it, and
//! each is told of its changes under the path it asked for. Adding and
//! removing watches happens on the caller's thread; the reading thread holds
//! the table only to look up who is told, and tells them after letting go,
//! so a handler may itself watch or stop watching.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};
use notify::event::{CreateKind, DataChange, Flag, MetadataKind, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventKind};

use super::{Handler, refused};

/// What a shared watch is told of: changes, not reads.
const MASK: WatchMask = WatchMask::ATTRIB
    .union(WatchMask::CREATE)
    .union(WatchMask::DELETE)
    .union(WatchMask::MODIFY)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

struct Hub {
    state: Mutex<State>,
}

struct State {
    watches: Watches,
    places: HashMap<WatchDescriptor, Place>,
    /// The last place each subscription saw something moved away from, to
    /// pair with where it arrives.
    moving: HashMap<u64, (u32, PathBuf)>,
}

/// A watched inode and everyone who asked for it.
struct Place {
    directory: bool,
    users: Vec<User>,
}

struct User {
    subscription: u64,
    /// The path this one asked for, which its events are named under.
    path: PathBuf,
    handler: Handler,
}

impl Hub {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The process's hub, made the first time one is needed. Where it cannot be
/// made, the next watch asks again.
fn hub() -> io::Result<Arc<Hub>> {
    static HUB: Mutex<Option<Arc<Hub>>> = Mutex::new(None);
    let mut slot = HUB.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(hub) = &*slot {
        return Ok(hub.clone());
    }
    let inotify = Inotify::init()?;
    let hub = Arc::new(Hub {
        state: Mutex::new(State { watches: inotify.watches(), places: HashMap::new(), moving: HashMap::new() }),
    });
    let reading = hub.clone();
    std::thread::Builder::new().name("fastctx-watch".into()).spawn(move || read(inotify, &reading))?;
    *slot = Some(hub.clone());
    Ok(hub)
}

/// One watcher's share of the hub.
pub(crate) struct Subscription {
    hub: Arc<Hub>,
    id: u64,
    handler: Handler,
    held: Vec<WatchDescriptor>,
}

impl Subscription {
    pub(crate) fn new(handler: Handler) -> notify::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let hub = hub().map_err(notify::Error::io)?;
        Ok(Self { hub, id: NEXT.fetch_add(1, Ordering::Relaxed), handler, held: Vec::new() })
    }

    pub(crate) fn watch(&mut self, path: &Path) -> notify::Result<()> {
        let mut state = self.hub.state();
        let watch = state.watches.add(path, MASK).map_err(|error| refused(error, path))?;
        let place =
            state.places.entry(watch.clone()).or_insert_with(|| Place { directory: path.is_dir(), users: Vec::new() });
        if place.users.iter().any(|user| user.subscription == self.id && user.path == path) {
            return Ok(());
        }
        place.users.push(User { subscription: self.id, path: path.to_path_buf(), handler: self.handler.clone() });
        self.held.push(watch);
        Ok(())
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut state = self.hub.state();
        for watch in self.held.drain(..) {
            let Some(place) = state.places.get_mut(&watch) else { continue };
            place.users.retain(|user| user.subscription != self.id);
            if place.users.is_empty() {
                state.places.remove(&watch);
                let _ = state.watches.remove(watch);
            }
        }
        state.moving.remove(&self.id);
    }
}

/// Reads the instance for as long as the process lives.
fn read(mut inotify: Inotify, hub: &Hub) {
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let told = match inotify.read_events_blocking(&mut buffer) {
            Ok(events) => route(&mut hub.state(), events),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                let told =
                    everyone(&hub.state(), || Err(notify::Error::io(io::Error::new(error.kind(), error.to_string()))));
                // Whatever broke the read is unlikely to mend at once.
                std::thread::sleep(Duration::from_secs(1));
                told
            }
        };
        for (handler, event) in told {
            handler.lock().unwrap_or_else(PoisonError::into_inner).handle_event(event);
        }
    }
}

type Told = Vec<(Handler, notify::Result<Event>)>;

/// Each subscription with a watch, told `event` once.
fn everyone(state: &State, event: impl Fn() -> notify::Result<Event>) -> Told {
    let mut told: Told = Vec::new();
    let mut seen = Vec::new();
    for user in state.places.values().flat_map(|place| &place.users) {
        if !seen.contains(&user.subscription) {
            seen.push(user.subscription);
            told.push((user.handler.clone(), event()));
        }
    }
    told
}

/// Who is told what of `events`, in `notify`'s words.
fn route<'a>(state: &mut State, events: impl Iterator<Item = inotify::Event<&'a std::ffi::OsStr>>) -> Told {
    let mut told = Told::new();
    for event in events {
        if event.mask.contains(EventMask::Q_OVERFLOW) {
            told.extend(everyone(state, || Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan))));
            continue;
        }
        if event.mask.contains(EventMask::IGNORED) {
            state.places.remove(&event.wd);
            continue;
        }
        let State { places, moving, .. } = state;
        let Some(place) = places.get(&event.wd) else { continue };
        let kind = |directory: bool| if directory { CreateKind::Folder } else { CreateKind::File };
        let gone = |directory: bool| if directory { RemoveKind::Folder } else { RemoveKind::File };
        let inside = event.mask.contains(EventMask::ISDIR);
        for user in &place.users {
            let path = match event.name {
                Some(name) => user.path.join(name),
                None => user.path.clone(),
            };
            let mut tell = |event: Event| told.push((user.handler.clone(), Ok(event)));
            if event.mask.contains(EventMask::MOVED_FROM) {
                moving.insert(user.subscription, (event.cookie, path.clone()));
                tell(
                    Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
                        .add_path(path.clone())
                        .set_tracker(event.cookie as usize),
                );
            }
            if event.mask.contains(EventMask::MOVED_TO) {
                tell(
                    Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::To)))
                        .add_path(path.clone())
                        .set_tracker(event.cookie as usize),
                );
                if moving.get(&user.subscription).is_some_and(|(cookie, _)| *cookie == event.cookie) {
                    let (_, from) = moving.remove(&user.subscription).expect("just found");
                    tell(
                        Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                            .add_path(from)
                            .add_path(path.clone())
                            .set_tracker(event.cookie as usize),
                    );
                }
            }
            if event.mask.contains(EventMask::MOVE_SELF) {
                tell(Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From))).add_path(path.clone()));
            }
            if event.mask.contains(EventMask::CREATE) {
                tell(Event::new(EventKind::Create(kind(inside))).add_path(path.clone()));
            }
            if event.mask.contains(EventMask::DELETE) {
                tell(Event::new(EventKind::Remove(gone(inside))).add_path(path.clone()));
            }
            if event.mask.contains(EventMask::DELETE_SELF) {
                tell(Event::new(EventKind::Remove(gone(place.directory))).add_path(path.clone()));
            }
            if event.mask.contains(EventMask::MODIFY) {
                tell(Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any))).add_path(path.clone()));
            }
            if event.mask.contains(EventMask::ATTRIB) {
                tell(Event::new(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))).add_path(path));
            }
        }
    }
    told
}
