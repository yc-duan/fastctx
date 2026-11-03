//! Projections put aside on disk behind their owner's back: written by a
//! thread of their own, never under the lock their owner keeps them by, and
//! read from memory until they are written. A path waits for the thread once,
//! with its latest bytes alone, however often it is put meanwhile, so what
//! waits is never more than one projection a path. A write that fails keeps
//! its bytes in memory, since its owner let go of them, is said once, and is
//! tried again a while later; nothing here ends the process.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::Duration;

use crate::roles::{self, Role};

/// How long a write that failed waits before it is tried again.
const AGAIN: Duration = Duration::from_secs(30);

pub struct Aside {
    shared: Arc<Mutex<Shared>>,
    telling: mpsc::Sender<PathBuf>,
}

#[derive(Default)]
struct Shared {
    /// On their way to disk, by path: the latest bytes and their place in line.
    writing: HashMap<PathBuf, (u64, Arc<Vec<u8>>)>,
    /// To be taken off the disk.
    removing: HashSet<PathBuf>,
    /// Told to the thread and not taken up by it yet.
    told: HashSet<PathBuf>,
    sent: u64,
    /// Why the last write failed, said once until it changes.
    trouble: Option<String>,
}

impl Aside {
    /// Puts projections aside on a thread called `name`, which ends when
    /// this is dropped. `what` names them in what is said of a failure.
    pub fn new(name: &str, what: &'static str) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (telling, told) = mpsc::channel();
        let writing = shared.clone();
        let _ = std::thread::Builder::new().name(name.to_string()).spawn(move || write(&told, &writing, what));
        Self { shared, telling }
    }

    /// Writes `bytes` to `path` in good time; until then they are
    /// `on_its_way`.
    pub fn put(&self, path: PathBuf, bytes: Vec<u8>) {
        let mut shared = self.shared.lock().expect("never poisoned");
        shared.sent += 1;
        let sent = shared.sent;
        shared.removing.remove(&path);
        shared.writing.insert(path.clone(), (sent, Arc::new(bytes)));
        self.tell(shared, path);
    }

    /// What is on its way to `path`, not written yet.
    pub fn on_its_way(&self, path: &Path) -> Option<Vec<u8>> {
        self.shared.lock().expect("never poisoned").writing.get(path).map(|(_, bytes)| bytes.to_vec())
    }

    /// Takes `path` away, with whatever was on its way to it.
    pub fn remove(&self, path: PathBuf) {
        let mut shared = self.shared.lock().expect("never poisoned");
        shared.writing.remove(&path);
        shared.removing.insert(path.clone());
        self.tell(shared, path);
    }

    /// Tells the thread of `path`, unless it was told and has not taken it up.
    fn tell(&self, mut shared: MutexGuard<'_, Shared>, path: PathBuf) {
        if shared.told.insert(path.clone()) {
            drop(shared);
            let _ = self.telling.send(path);
        }
    }
}

fn write(told: &mpsc::Receiver<PathBuf>, shared: &Mutex<Shared>, what: &str) {
    roles::set(Role::Blocking);
    loop {
        match told.recv_timeout(AGAIN) {
            Ok(path) => settle(path, shared, what),
            // Each write that failed, untold since, is tried again.
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let failed: Vec<PathBuf> = {
                    let shared = shared.lock().expect("never poisoned");
                    shared.writing.keys().filter(|path| !shared.told.contains(*path)).cloned().collect()
                };
                for path in failed {
                    settle(path, shared, what);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Writes `path` as it stands now, or takes it off the disk.
fn settle(path: PathBuf, shared: &Mutex<Shared>, what: &str) {
    let (waiting, removing) = {
        let mut shared = shared.lock().expect("never poisoned");
        shared.told.remove(&path);
        match shared.writing.get(&path) {
            Some((sent, bytes)) => (Some((*sent, bytes.clone())), false),
            None => (None, shared.removing.remove(&path)),
        }
    };
    if let Some((sent, bytes)) = waiting {
        let written = crate::publish(&path, &bytes, crate::Permissions::Private);
        let mut shared = shared.lock().expect("never poisoned");
        match written {
            Ok(()) => {
                if shared.writing.get(&path).is_some_and(|(current, _)| *current == sent) {
                    shared.writing.remove(&path);
                }
                shared.trouble = None;
            }
            Err(error) => {
                let trouble = error.to_string();
                if shared.trouble.as_ref() != Some(&trouble) {
                    eprintln!("fastctx: putting {what} aside: {trouble}; it stays in memory and is tried again");
                    shared.trouble = Some(trouble);
                }
            }
        }
    } else if removing {
        match crate::disk::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => eprintln!("fastctx: removing {what}: {error}"),
            _ => {}
        }
    }
}
