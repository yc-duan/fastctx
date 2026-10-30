//! One inotify instance and one thread for every watch in the process.
//!
//! inotify gives one watch per inode in an instance, whoever asks: a place
//! two watchers watch is one watch, kept while either of them still wants it,
//! and each is told of its changes under the path it asked for. A recursive
//! watch is one watch per directory, added as directories appear and dropped
//! as they go, into the directories its watcher keeps only. Adding and
//! removing watches happens on the caller's thread; the reading thread holds
//! the table only to look up who is told and to follow a tree, and tells them
//! after letting go, so a handler may itself watch or stop watching.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};
use notify::event::{CreateKind, DataChange, Flag, MetadataKind, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventKind};

use super::{Handler, Root, refused};

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
    subscriptions: HashMap<u64, Sub>,
}

/// One watcher: who it tells, and the places it asked for.
struct Sub {
    handler: Handler,
    roots: Vec<Root>,
    /// The last place it saw something moved away from, to pair with where
    /// it arrives.
    moving: Option<(u32, PathBuf)>,
    /// The parts of its trees the system had no watches left for, looked at
    /// every so often instead (`crate::poll`), for as long as it lives.
    scanned: Option<crate::poll::Polling>,
}

/// A watched inode and every root it is watched for.
struct Place {
    directory: bool,
    users: Vec<User>,
}

#[derive(Clone, PartialEq)]
struct User {
    subscription: u64,
    root: usize,
    /// The path the inode has under that root, which its events are named
    /// under.
    path: PathBuf,
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
        state: Mutex::new(State { watches: inotify.watches(), places: HashMap::new(), subscriptions: HashMap::new() }),
    });
    let reading = hub.clone();
    std::thread::Builder::new().name("fastctx-watch".into()).spawn(move || read(inotify, &reading))?;
    *slot = Some(hub.clone());
    Ok(hub)
}

/// One watcher's share of the hub. Dropping it ends its watches.
pub(crate) struct Subscription {
    hub: Arc<Hub>,
    id: u64,
}

impl Subscription {
    pub(crate) fn new(handler: Handler) -> notify::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let hub = hub().map_err(notify::Error::io)?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        hub.state().subscriptions.insert(id, Sub { handler, roots: Vec::new(), moving: None, scanned: None });
        Ok(Self { hub, id })
    }

    /// Watches `root`; where not all of it could be watched, none of it is.
    pub(crate) fn watch(&mut self, root: Root) -> notify::Result<()> {
        let mut state = self.hub.state();
        let sub = state.subscriptions.get_mut(&self.id).expect("a live subscription");
        let index = sub.roots.len();
        let path = root.path.clone();
        sub.roots.push(root);
        let added = add(&mut state, self.id, index, &path).and_then(|()| grow(&mut state, self.id, index, &path));
        if added.is_err() {
            prune(&mut state, |user| user.subscription == self.id && user.root == index);
        }
        added
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut state = self.hub.state();
        prune(&mut state, |user| user.subscription == self.id);
        state.subscriptions.remove(&self.id);
    }
}

/// Watches the one place `path` for the root `index` of `subscription`.
fn add(state: &mut State, subscription: u64, index: usize, path: &Path) -> notify::Result<()> {
    let watch = state.watches.add(path, MASK).map_err(|error| refused(error, path))?;
    let place = state.places.entry(watch).or_insert_with(|| Place { directory: path.is_dir(), users: Vec::new() });
    let user = User { subscription, root: index, path: path.to_path_buf() };
    if !place.users.contains(&user) {
        place.users.push(user);
    }
    Ok(())
}

/// Watches the directories under `directory` that the recursive root
/// `index` keeps, as they are now.
fn grow(state: &mut State, subscription: u64, index: usize, directory: &Path) -> notify::Result<()> {
    let root = state.subscriptions[&subscription].roots[index].clone();
    if !root.recursive {
        return Ok(());
    }
    let mut seen = HashSet::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for inner in crate::directories(&directory, root.follow_links, &mut seen) {
            if root.keeps(&inner) {
                add(state, subscription, index, &inner)?;
                pending.push(inner);
            }
        }
    }
    Ok(())
}

/// Looks every so often at the tree under `path` of the root `index` of
/// `subscription`, which the system had no watches left for as it grew or
/// after it lost events: the watches it holds there go, and the looking
/// tells of what changes under it, so nothing there goes unseen.
fn scan_instead(state: &mut State, subscription: u64, index: usize, path: &Path) -> notify::Result<()> {
    prune(state, |user| user.subscription == subscription && user.root == index && user.path.starts_with(path));
    let sub = state.subscriptions.get_mut(&subscription).expect("a live subscription");
    let root = sub.roots[index].clone();
    let scanned = match &mut sub.scanned {
        Some(scanned) => scanned,
        None => sub.scanned.insert(crate::poll::Polling::new(sub.handler.clone())?),
    };
    scanned.watch(Root { path: path.to_path_buf(), ..root })
}

/// Stops the watches of the users `drop` picks, and every watch no one is
/// left on.
fn prune(state: &mut State, drop: impl Fn(&User) -> bool) {
    let State { places, watches, .. } = state;
    places.retain(|watch, place| {
        place.users.retain(|user| !drop(user));
        if place.users.is_empty() {
            let _ = watches.remove(watch.clone());
        }
        !place.users.is_empty()
    });
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
    state.subscriptions.values().map(|sub| (sub.handler.clone(), event())).collect()
}

/// Who is told what of `events`, in `notify`'s words, and the trees that
/// grew or shrank with them followed.
fn route<'a>(state: &mut State, events: impl Iterator<Item = inotify::Event<&'a std::ffi::OsStr>>) -> Told {
    let mut told = Told::new();
    let mut grown = Vec::new();
    let mut gone = Vec::new();
    let mut overflowed = false;
    for event in events {
        if event.mask.contains(EventMask::Q_OVERFLOW) {
            told.extend(everyone(state, || Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan))));
            overflowed = true;
            continue;
        }
        if event.mask.contains(EventMask::IGNORED) {
            state.places.remove(&event.wd);
            continue;
        }
        let State { places, subscriptions, .. } = state;
        let Some(place) = places.get(&event.wd) else { continue };
        let inside = event.mask.contains(EventMask::ISDIR);
        let made = |directory: bool| if directory { CreateKind::Folder } else { CreateKind::File };
        let removed = |directory: bool| if directory { RemoveKind::Folder } else { RemoveKind::File };
        for user in &place.users {
            let Some(sub) = subscriptions.get_mut(&user.subscription) else { continue };
            let root = &sub.roots[user.root];
            let own = user.path == root.path;
            let path = match event.name {
                Some(name) => user.path.join(name),
                None => user.path.clone(),
            };
            if inside && root.keeps(&path) {
                if event.mask.intersects(EventMask::CREATE | EventMask::MOVED_TO) {
                    grown.push((user.subscription, user.root, path.clone()));
                }
                if event.mask.intersects(EventMask::DELETE | EventMask::MOVED_FROM) {
                    gone.push((user.subscription, user.root, path.clone()));
                }
            }
            let handler = sub.handler.clone();
            let mut tell = |event: Event| told.push((handler.clone(), Ok(event)));
            if event.mask.contains(EventMask::MOVED_FROM) {
                sub.moving = Some((event.cookie, path.clone()));
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
                if let Some((_, from)) = sub.moving.take_if(|(cookie, _)| *cookie == event.cookie) {
                    tell(
                        Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                            .add_path(from)
                            .add_path(path.clone())
                            .set_tracker(event.cookie as usize),
                    );
                }
            }
            // A directory inside a tree is told of by its parent, not itself.
            if own && event.mask.contains(EventMask::MOVE_SELF) {
                tell(Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From))).add_path(path.clone()));
            }
            if event.mask.contains(EventMask::CREATE) {
                tell(Event::new(EventKind::Create(made(inside))).add_path(path.clone()));
            }
            if event.mask.contains(EventMask::DELETE) {
                tell(Event::new(EventKind::Remove(removed(inside))).add_path(path.clone()));
            }
            if own && event.mask.contains(EventMask::DELETE_SELF) {
                tell(Event::new(EventKind::Remove(removed(place.directory))).add_path(path.clone()));
            }
            if event.mask.contains(EventMask::MODIFY) {
                tell(Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any))).add_path(path.clone()));
            }
            if event.mask.contains(EventMask::ATTRIB) {
                tell(Event::new(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))).add_path(path));
            }
        }
    }
    // A directory moved away keeps its watches under its old name: they go,
    // and come again under the new one where it arrived inside a tree.
    for (subscription, index, path) in gone {
        prune(state, |user| user.subscription == subscription && user.root == index && user.path.starts_with(&path));
    }
    for (subscription, index, path) in grown {
        if !state.subscriptions.contains_key(&subscription) {
            continue;
        }
        let added = add(state, subscription, index, &path).and_then(|()| grow(state, subscription, index, &path));
        if let Err(error) = added {
            // What could not be watched is said, so the watcher looks again
            // at what it holds, and is looked at every so often from now on.
            let handler = state.subscriptions[&subscription].handler.clone();
            told.push((handler.clone(), Err(error)));
            if let Err(error) = scan_instead(state, subscription, index, &path) {
                told.push((handler, Err(error)));
            }
        }
    }
    // Events were lost: directories made meanwhile may have no watch yet.
    // Each tree is followed again from its root; one the system has no
    // watches left for is looked at instead.
    if overflowed {
        let trees: Vec<(u64, usize, PathBuf)> = state
            .subscriptions
            .iter()
            .flat_map(|(id, sub)| {
                sub.roots
                    .iter()
                    .enumerate()
                    .filter(|(_, root)| root.recursive)
                    .map(|(index, root)| (*id, index, root.path.clone()))
            })
            .collect();
        for (subscription, index, path) in trees {
            if let Err(error) = grow(state, subscription, index, &path) {
                let handler = state.subscriptions[&subscription].handler.clone();
                told.push((handler.clone(), Err(error)));
                if let Err(error) = scan_instead(state, subscription, index, &path) {
                    told.push((handler, Err(error)));
                }
            }
        }
    }
    told
}
