//! One business handoff per immutable letter identity, shared by every hub.
//! Its receipt settles once the place its letter came at is acknowledged to
//! that hub (`fastctx_storage::receipts`); legacy files carry no such proof.

use fastctx_storage::{
    Database,
    receipts::{self, Key},
};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

pub(super) const KEEP: Duration = Duration::from_secs(7 * 86400);
type Gates = HashMap<PathBuf, Weak<Gate>>;
static ACTIVE: LazyLock<Mutex<Gates>> = LazyLock::new(Default::default);

struct Gate {
    path: PathBuf,
    lock: Arc<AsyncMutex<()>>,
}

impl Drop for Gate {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().expect("never poisoned");
        if active.get(&self.path).is_some_and(|gate| std::ptr::eq(gate.as_ptr(), self)) {
            active.remove(&self.path);
        }
        if active.is_empty() {
            *active = HashMap::new();
        }
    }
}

pub(crate) struct Handoff {
    path: PathBuf,
    database: Database,
    key: Key,
    _held: OwnedMutexGuard<()>,
    _gate: Arc<Gate>,
}

pub(super) fn key(id: &str) -> Key {
    Key::new("letter-business", "", id)
}

impl Handoff {
    pub async fn begin(node: &crate::Node, identity: &super::Identity) -> Self {
        let path = node.world.path().join("letter-receipts").join(identity.id.as_str());
        let gate = {
            let mut active = ACTIVE.lock().expect("never poisoned");
            match active.get(&path).and_then(Weak::upgrade) {
                Some(gate) => gate,
                None => {
                    let gate = Arc::new(Gate { path: path.clone(), lock: Arc::new(AsyncMutex::new(())) });
                    active.insert(path.clone(), Arc::downgrade(&gate));
                    gate
                }
            }
        };
        let held = gate.lock.clone().lock_owned().await;
        Self { path, database: node.database.clone(), key: key(identity.id.as_str()), _held: held, _gate: gate }
    }

    pub async fn finished(&self) -> io::Result<bool> {
        let key = self.key.clone();
        if self
            .database
            .read(move |connection| receipts::get(connection, &key))
            .await
            .map_err(io::Error::other)?
            .is_some()
        {
            return Ok(true);
        }
        match std::fs::read(&self.path) {
            Ok(bytes) if bytes == b"accepted" => {
                let key = self.key.clone();
                self.database
                    .write(key.bytes() + 8, move |connection| {
                        receipts::import(connection, &key, b"accepted", KEEP, jiff::Timestamp::now().as_millisecond())
                    })
                    .await
                    .map_err(io::Error::other)?;
                Ok(true)
            }
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid letter receipt: {}", self.path.display()),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Migration may have committed and removed the file between
                // the first indexed read and the fallback file read.
                let key = self.key.clone();
                self.database
                    .read(move |connection| receipts::get(connection, &key).map(|value| value.is_some()))
                    .await
                    .map_err(io::Error::other)
            }
            Err(error) => Err(error),
        }
    }

    /// Keeps that the letter was handed over; `received`, where it came as
    /// a letter, is what settles the receipt once acknowledged.
    pub async fn finish(&self, received: Option<&receipts::Reference>) -> io::Result<()> {
        let key = self.key.clone();
        let received = received.cloned();
        self.database
            .write(key.bytes() + 8, move |connection| {
                receipts::put(
                    connection,
                    &key,
                    b"accepted",
                    KEEP,
                    false,
                    received.as_ref(),
                    jiff::Timestamp::now().as_millisecond(),
                )
            })
            .await
            .map_err(io::Error::other)
    }
}
