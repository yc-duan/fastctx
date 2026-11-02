//! Projections put aside on disk behind their owner's back: written by a
//! thread of their own, never under the lock their owner keeps them by, and
//! read from memory until they are written. A write that fails keeps its
//! bytes in memory and is said once; nothing here ends the process.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use crate::roles::{self, Role};

pub struct Aside {
    shared: Arc<Mutex<Shared>>,
    putting: mpsc::Sender<Put>,
}

#[derive(Default)]
struct Shared {
    /// On their way to disk, by path: the bytes and their place in line.
    writing: HashMap<PathBuf, (u64, Vec<u8>)>,
    sent: u64,
    /// Why the last write failed, said once until it changes.
    trouble: Option<String>,
}

enum Put {
    Write { path: PathBuf, sent: u64, bytes: Vec<u8> },
    Remove { path: PathBuf },
}

impl Aside {
    /// Puts projections aside on a thread called `name`, which ends when
    /// this is dropped. `what` names them in what is said of a failure.
    pub fn new(name: &str, what: &'static str) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (putting, puts) = mpsc::channel();
        let writing = shared.clone();
        let _ = std::thread::Builder::new().name(name.to_string()).spawn(move || put(&puts, &writing, what));
        Self { shared, putting }
    }

    /// Writes `bytes` to `path` in good time; until then they are
    /// `on_its_way`.
    pub fn put(&self, path: PathBuf, bytes: Vec<u8>) {
        let mut shared = self.shared.lock().expect("never poisoned");
        shared.sent += 1;
        let sent = shared.sent;
        shared.writing.insert(path.clone(), (sent, bytes.clone()));
        drop(shared);
        let _ = self.putting.send(Put::Write { path, sent, bytes });
    }

    /// What is on its way to `path`, not written yet.
    pub fn on_its_way(&self, path: &Path) -> Option<Vec<u8>> {
        self.shared.lock().expect("never poisoned").writing.get(path).map(|(_, bytes)| bytes.clone())
    }

    /// Takes `path` away, with whatever was on its way to it.
    pub fn remove(&self, path: PathBuf) {
        self.shared.lock().expect("never poisoned").writing.remove(&path);
        let _ = self.putting.send(Put::Remove { path });
    }
}

fn put(puts: &mpsc::Receiver<Put>, shared: &Mutex<Shared>, what: &str) {
    roles::set(Role::Blocking);
    for put in puts {
        match put {
            Put::Write { path, sent, bytes } => {
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
                            eprintln!("fastctx: putting {what} aside: {trouble}; it stays in memory");
                            shared.trouble = Some(trouble);
                        }
                    }
                }
            }
            Put::Remove { path } => match crate::disk::remove_file(&path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    eprintln!("fastctx: removing {what}: {error}");
                }
                _ => {}
            },
        }
    }
}
