//! Connection-owned searches and the idle leases of legacy callers.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, Weak};

use fastctx_file_search::Search;
use fastctx_types::MachineName;
use fastctx_types::file_search::{FileSearchResult, FileSearchScope, START_IDLE_TIMEOUT};
use fastctx_types::ipc::Failure;
use tokio::sync::watch;
use tokio::time::Instant;

type Key = (MachineName, String);
type Sessions = Mutex<HashMap<Key, Entry>>;

struct Entry {
    session: Arc<Session>,
    expires: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct Searches(Arc<Sessions>);

pub(super) struct Session {
    pub scope: FileSearchScope,
    state: Mutex<SessionIndex>,
    closed: watch::Sender<bool>,
}

struct SessionIndex {
    roots: Vec<String>,
    index: Arc<Search>,
}

impl Session {
    pub fn new(scope: FileSearchScope, roots: Vec<String>) -> io::Result<Self> {
        let index = Arc::new(Search::new(roots.clone())?);
        Ok(Self { scope, state: Mutex::new(SessionIndex { roots, index }), closed: watch::channel(false).0 })
    }

    fn check_open(&self) -> io::Result<()> {
        if *self.closed.borrow() {
            Err(io::Error::new(io::ErrorKind::Interrupted, "file search is closed or belongs to another caller"))
        } else {
            Ok(())
        }
    }

    pub fn query(&self, roots: Vec<String>, query: String, limit: usize) -> io::Result<FileSearchResult> {
        let (index, replaced) = {
            let mut state = self.state.lock().expect("search session poisoned");
            self.check_open()?;
            let replaced = if state.roots != roots {
                let index = Arc::new(Search::new(roots.clone())?);
                if let Err(error) = self.check_open() {
                    drop(state);
                    drop(index);
                    return Err(error);
                }
                state.roots = roots;
                Some(std::mem::replace(&mut state.index, index))
            } else {
                None
            };
            (state.index.clone(), replaced)
        };
        // Retired snapshots may still have readers. Stop their scanners and
        // matching too, and join only after releasing the session's lock.
        if let Some(replaced) = replaced {
            replaced.close();
            drop(replaced);
        }
        let result = index.query(query.clone(), limit);
        self.check_open()?;
        match result {
            // Another query with newer roots retired this snapshot while it
            // ranked: not ready, so the caller asks again.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                Ok(FileSearchResult::Matches { query, ready: false, total: 0, files: vec![] })
            }
            result => result,
        }
    }

    fn close(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        self.closed.send_replace(true);
        tokio::task::spawn_blocking(move || {
            self.state.lock().expect("search session poisoned").index.close();
            drop(self);
        })
    }
}

impl Searches {
    pub(super) fn insert(&self, key: Key, session: Session) -> Lease {
        let ended = session.closed.subscribe();
        self.0
            .lock()
            .expect("searches poisoned")
            .insert(key.clone(), Entry { session: Arc::new(session), expires: None });
        Lease { sessions: Arc::downgrade(&self.0), key: Some(key), ended }
    }

    pub(super) fn get(&self, key: &Key) -> Result<Arc<Session>, Failure> {
        let mut sessions = self.0.lock().expect("searches poisoned");
        let now = Instant::now();
        if let Some(entry) = sessions.get_mut(key) {
            if entry.expires.is_none_or(|expires| now < expires) {
                if entry.expires.is_some() {
                    entry.expires = Some(now + START_IDLE_TIMEOUT);
                }
                return Ok(entry.session.clone());
            }
            let entry = sessions.remove(key).expect("expired search is present");
            drop(sessions);
            entry.session.close();
        }
        Err(Failure::new("file search is closed or belongs to another caller"))
    }

    pub(super) fn close(&self, key: &Key) -> Option<tokio::task::JoinHandle<()>> {
        close(&self.0, key)
    }
}

fn close(sessions: &Sessions, key: &Key) -> Option<tokio::task::JoinHandle<()>> {
    let entry = sessions.lock().expect("searches poisoned").remove(key);
    entry.map(|entry| entry.session.close())
}

pub(crate) struct Lease {
    sessions: Weak<Sessions>,
    key: Option<Key>,
    ended: watch::Receiver<bool>,
}

impl Lease {
    pub(super) async fn closed(&mut self) {
        let _ = self.ended.wait_for(|closed| *closed).await;
    }

    /// Transfer ownership to the receiver's idle timer before returning an ID.
    pub(super) fn keep_until_idle(mut self) {
        let Some(sessions) = self.sessions.upgrade() else { return };
        let key = self.key.as_ref().expect("connection owns the search");
        let expires = Instant::now() + START_IDLE_TIMEOUT;
        {
            let mut sessions = sessions.lock().expect("searches poisoned");
            let Some(entry) = sessions.get_mut(key) else { return };
            entry.expires = Some(expires);
        }
        tokio::spawn(expire(self.sessions.clone(), key.clone(), self.ended.clone(), expires));
        self.key = None;
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(key) = &self.key
            && let Some(sessions) = self.sessions.upgrade()
        {
            close(&sessions, key);
        }
    }
}

async fn expire(sessions: Weak<Sessions>, key: Key, mut ended: watch::Receiver<bool>, mut expires: Instant) {
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(expires) => {},
            _ = ended.wait_for(|closed| *closed) => return,
        }
        let Some(sessions) = sessions.upgrade() else { return };
        let expired = {
            let mut sessions = sessions.lock().expect("searches poisoned");
            let Some(entry) = sessions.get(&key) else { return };
            expires = entry.expires.expect("legacy search has an idle deadline");
            if Instant::now() < expires {
                continue;
            }
            sessions.remove(&key).expect("expired search is present")
        };
        expired.session.close();
        return;
    }
}
