//! Files the hub rewrites whole as what they record changes, written by
//! more than one task: the records of where every member's letters stand
//! (`letters.json`), and each sender's count of its own letters. Every write
//! carries the revision of what it holds (`crate::store::Store::revision`),
//! and one no newer than the last written is passed over, so no write that
//! comes late takes a file back to what it was.

use std::io;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::store::{letter_rows::Snapshot, shelf::Shelf};
use tokio::sync::watch;

/// Lets through only writes newer than the last one let through.
pub(crate) struct Fence {
    written: Mutex<u64>,
}

impl Fence {
    pub(crate) fn new() -> Self {
        Self { written: Mutex::new(0) }
    }

    /// Writes with `write` what is as of `revision`, unless what was
    /// written already is as new; whether it wrote. Writes through one
    /// fence never overlap.
    pub(crate) fn write(&self, revision: u64, write: impl FnOnce() -> io::Result<()>) -> io::Result<bool> {
        let mut written = self.written.lock().expect("never poisoned");
        if revision <= *written {
            return Ok(false);
        }
        write()?;
        *written = revision;
        Ok(true)
    }

    /// Takes no write any more: what the file recorded is gone.
    pub(crate) fn close(&self) {
        *self.written.lock().expect("never poisoned") = u64::MAX;
    }
}

/// The records of where every member's letters stand, and how far they
/// are written.
pub(crate) struct Records {
    fence: Fence,
    said: watch::Sender<u64>,
    changes: AtomicU64,
}

impl Records {
    pub(crate) fn new() -> Self {
        Self { fence: Fence::new(), said: watch::channel(0).0, changes: AtomicU64::new(0) }
    }

    /// Writes `bytes`, the records as of `revision`, to `path`, unless newer
    /// ones are written already.
    pub(crate) fn write(&self, revision: u64, shelf: &Shelf, snapshot: &Snapshot) -> io::Result<()> {
        let changes = snapshot.changes()?;
        if self.fence.write(revision, || {
            shelf.write(changes)?;
            self.changes.store(snapshot.revision, Ordering::Release);
            Ok(())
        })? {
            self.said.send_if_modified(|said| {
                let newer = revision > *said;
                if newer {
                    *said = revision;
                }
                newer
            });
        }
        Ok(())
    }

    pub(crate) fn written_changes(&self) -> u64 {
        self.changes.load(Ordering::Acquire)
    }

    /// The revision of the records written last.
    pub(crate) fn written_revision(&self) -> u64 {
        *self.said.borrow()
    }

    /// Resolves once the records of `revision`, or newer ones, are written.
    pub(crate) async fn written(&self, revision: u64) {
        let mut said = self.said.subscribe();
        let _ = said.wait_for(|said| *said >= revision).await;
    }
}
