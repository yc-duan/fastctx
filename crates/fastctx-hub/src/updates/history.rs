//! Last reports are keyed by identity. A burst retains only each changed
//! device's latest report, and commits those rows through the hub's writer.

use crate::store::{
    rows::{Changes, Put},
    shelf::Shelf,
};
use fastctx_net::sign::PublicKey;
use fastctx_types::update::Report;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{Notify, watch};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct LastReport {
    pub at: Timestamp,
    pub report: Report,
}
pub(super) type History = BTreeMap<PublicKey, Arc<LastReport>>;

#[derive(Clone)]
pub(super) struct Writer(Arc<Writing>);
impl Drop for Writer {
    fn drop(&mut self) {
        self.0.changed.notify_one();
    }
}
struct Writing {
    pending: Mutex<Pending>,
    changed: Notify,
    written: watch::Sender<u64>,
}
#[derive(Default)]
struct Pending {
    revision: u64,
    entries: BTreeMap<PublicKey, Option<Arc<LastReport>>>,
}

impl Writer {
    pub(super) fn put(&self, key: PublicKey, value: Option<Arc<LastReport>>) {
        let mut pending = self.0.pending.lock().expect("never poisoned");
        pending.revision = pending.revision.checked_add(1).expect("report revision exhausted");
        pending.entries.insert(key, value);
        self.0.changed.notify_one();
    }
    pub(super) async fn flush(&self) {
        let revision = self.0.pending.lock().expect("never poisoned").revision;
        let mut written = self.0.written.subscribe();
        self.0.changed.notify_one();
        let _ = written.wait_for(|written| *written >= revision).await;
    }
}

pub(super) fn open(shelf: &Shelf) -> Result<(History, Writer), String> {
    let mut last = History::new();
    for (key, _, value) in shelf.rows("reports").map_err(|e| e.to_string())? {
        last.insert(key.parse().map_err(|e| format!("invalid report identity: {e}"))?, value);
    }
    let writer = Writer(Arc::new(Writing {
        pending: Mutex::new(Pending::default()),
        changed: Notify::new(),
        written: watch::channel(0).0,
    }));
    let weak = Arc::downgrade(&writer.0);
    let shelf = shelf.clone();
    tokio::spawn(async move {
        loop {
            let Some(writing) = weak.upgrade() else { return };
            writing.changed.notified().await;
            let mut delay = std::time::Duration::from_secs(1);
            loop {
                let (revision, entries) = {
                    let mut pending = writing.pending.lock().expect("never poisoned");
                    (pending.revision, std::mem::take(&mut pending.entries))
                };
                if entries.is_empty() {
                    if Arc::strong_count(&writing) == 1 {
                        return;
                    }
                    break;
                }
                let (saving, records) = (shelf.clone(), entries.clone());
                let result = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                    let mut changes = Changes::default();
                    let mut bytes = 0;
                    for (key, value) in &records {
                        match value {
                            Some(value) => {
                                let body = serde_json::to_vec(value)?;
                                bytes += body.len() + 128;
                                changes.put.push(Put {
                                    kind: "reports",
                                    key: key.to_string().into(),
                                    position: 0,
                                    body,
                                });
                            }
                            None => {
                                bytes += 128;
                                changes.deleted.push(("reports", key.to_string().into()));
                            }
                        }
                        if bytes >= 4 * 1024 * 1024 {
                            saving.write(std::mem::take(&mut changes))?;
                            bytes = 0;
                        }
                    }
                    saving.write(changes)
                })
                .await;
                match result {
                    Ok(Ok(())) => {
                        writing.written.send_replace(revision);
                        if Arc::strong_count(&writing) == 1 {
                            return;
                        }
                        break;
                    }
                    error => {
                        eprintln!("keeping update reports: {error:?}");
                        {
                            let mut pending = writing.pending.lock().expect("never poisoned");
                            for (key, value) in entries {
                                pending.entries.entry(key).or_insert(value);
                            }
                        }
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(std::time::Duration::from_secs(60));
                    }
                }
            }
        }
    });
    Ok((last, writer))
}
