//! Looking at places every so often, for the watches the system had nothing
//! left for: one thread for the whole process, told of what changed between
//! two looks as created, removed or modified, under the paths asked for.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind};
use notify::{Event, EventKind};

use super::{Handler, POLL, Root};

/// What a look saw of one path.
#[derive(Clone, Copy, PartialEq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    directory: bool,
}

struct Polled {
    owner: u64,
    handler: Handler,
    root: Root,
    seen: HashMap<PathBuf, Stamp>,
}

struct Poller {
    polled: Mutex<Vec<Polled>>,
}

impl Poller {
    fn polled(&self) -> MutexGuard<'_, Vec<Polled>> {
        self.polled.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn poller() -> io::Result<Arc<Poller>> {
    static POLLER: Mutex<Option<Arc<Poller>>> = Mutex::new(None);
    let mut slot = POLLER.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(poller) = &*slot {
        return Ok(poller.clone());
    }
    let poller = Arc::new(Poller { polled: Mutex::new(Vec::new()) });
    let looking = poller.clone();
    std::thread::Builder::new().name("fastctx-watch-poll".into()).spawn(move || look(&looking))?;
    *slot = Some(poller.clone());
    Ok(poller)
}

/// One watcher's places that are looked at. Dropping it stops the looking.
pub(crate) struct Polling {
    poller: Arc<Poller>,
    owner: u64,
    handler: Handler,
}

impl Polling {
    pub(crate) fn new(handler: Handler) -> notify::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Ok(Self { poller: poller().map_err(notify::Error::io)?, owner: NEXT.fetch_add(1, Ordering::Relaxed), handler })
    }

    pub(crate) fn watch(&mut self, root: Root) -> notify::Result<()> {
        if !root.path.exists() {
            return Err(notify::Error::path_not_found().add_path(root.path));
        }
        let seen = scan(&root);
        self.poller.polled().push(Polled { owner: self.owner, handler: self.handler.clone(), root, seen });
        Ok(())
    }
}

impl Drop for Polling {
    fn drop(&mut self) {
        self.poller.polled().retain(|polled| polled.owner != self.owner);
    }
}

fn look(poller: &Poller) {
    loop {
        std::thread::sleep(POLL);
        let mut told = Vec::new();
        for polled in poller.polled().iter_mut() {
            let now = scan(&polled.root);
            for (path, stamp) in &now {
                let kind = match polled.seen.get(path) {
                    None if stamp.directory => EventKind::Create(CreateKind::Folder),
                    None => EventKind::Create(CreateKind::File),
                    Some(before) if before != stamp => EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                    Some(_) => continue,
                };
                told.push((polled.handler.clone(), Event::new(kind).add_path(path.clone())));
            }
            for (path, stamp) in &polled.seen {
                if !now.contains_key(path) {
                    let kind = if stamp.directory { RemoveKind::Folder } else { RemoveKind::File };
                    told.push((polled.handler.clone(), Event::new(EventKind::Remove(kind)).add_path(path.clone())));
                }
            }
            polled.seen = now;
        }
        for (handler, event) in told {
            handler.lock().unwrap_or_else(PoisonError::into_inner).handle_event(Ok(event));
        }
    }
}

/// The root and what is in it, as a non-recursive watch sees it: its
/// entries; a recursive one sees the entries of the directories it keeps.
fn scan(root: &Root) -> HashMap<PathBuf, Stamp> {
    let mut seen = HashMap::new();
    let Some(stamp) = stamp_of(&root.path) else { return seen };
    seen.insert(root.path.clone(), stamp);
    if !stamp.directory {
        return seen;
    }
    let mut followed = HashSet::new();
    let mut pending = vec![root.path.clone()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(stamp) = stamp_of(&path) else { continue };
            seen.insert(path.clone(), stamp);
            if stamp.directory && root.keeps(&path) && crate::enter(&path, root.follow_links, &mut followed) {
                pending.push(path);
            }
        }
    }
    seen
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { modified: meta.modified().ok(), len: meta.len(), directory: meta.is_dir() })
}
