//! A burst of version reports produces one publication. The first notice
//! runs immediately; further changes merge while it runs and between rounds.
use crate::Hub;
use fastctx_types::MachineName;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, watch};

pub(super) struct Publishing {
    pending: Mutex<Pending>,
    changed: Arc<Notify>,
    written: watch::Sender<u64>,
}

#[derive(Default)]
struct Pending {
    revision: u64,
    batch: Batch,
}

#[derive(Default)]
struct Batch {
    all: bool,
    initial: BTreeSet<MachineName>,
    prepare: bool,
}

impl Default for Publishing {
    fn default() -> Self {
        Self { pending: Mutex::default(), changed: Arc::default(), written: watch::channel(0).0 }
    }
}

impl Drop for Publishing {
    fn drop(&mut self) {
        self.changed.notify_one();
    }
}

impl Publishing {
    pub(super) fn request(&self, only: Option<&MachineName>, prepare: bool) {
        let mut pending = self.pending.lock().expect("never poisoned");
        pending.revision = pending.revision.checked_add(1).expect("version publication revision exhausted");
        if let Some(name) = only {
            pending.batch.initial.insert(name.clone());
        } else {
            pending.batch.all = true;
        }
        pending.batch.prepare |= prepare;
        self.changed.notify_one();
    }

    pub(super) async fn flush(&self) {
        let revision = self.pending.lock().expect("never poisoned").revision;
        let mut written = self.written.subscribe();
        let _ = written.wait_for(|written| *written >= revision).await;
    }
}

pub(crate) fn start(hub: &Arc<Hub>) {
    let publishing = hub.updates.lock().expect("never poisoned").publishing.clone();
    let changed = publishing.changed.clone();
    let state = Arc::downgrade(&publishing);
    let weak = Arc::downgrade(hub);
    tokio::spawn(async move {
        let mut next = tokio::time::Instant::now();
        loop {
            changed.notified().await;
            tokio::time::sleep_until(next).await;
            let Some(hub) = weak.upgrade() else { return };
            let Some(publishing) = state.upgrade() else { return };
            let (revision, batch) = {
                let mut pending = publishing.pending.lock().expect("never poisoned");
                (pending.revision, std::mem::take(&mut pending.batch))
            };
            let log = hub.log.clone();
            let result = tokio::task::spawn_blocking(move || {
                if batch.prepare {
                    super::prefetch(&hub);
                }
                super::announce::tell(&hub, batch.all, &batch.initial);
            })
            .await;
            match result {
                Ok(()) => {
                    publishing.written.send_replace(revision);
                }
                Err(error) => {
                    log.line(format_args!("publishing network versions: {error}"));
                    publishing.request(None, true);
                }
            }
            next = tokio::time::Instant::now() + Duration::from_millis(20);
        }
    });
}
