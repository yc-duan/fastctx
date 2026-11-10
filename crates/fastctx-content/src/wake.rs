//! Who waits for a stream is told when it moves, and only then: nothing is
//! read again on a timer.

use crate::{Content, Result, StreamKey};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::watch;

/// Past this many streams waited for, those no one waits for any more are let go.
const KEPT: usize = 1024;

#[derive(Default)]
pub(crate) struct Wakes {
    streams: Mutex<HashMap<StreamKey, watch::Sender<u64>>>,
}

impl Wakes {
    /// Tells whoever waits for `stream` that it is at `revision`.
    pub(crate) fn moved(&self, stream: &StreamKey, revision: u64) {
        let mut streams = self.streams.lock().expect("stream waiters");
        let Some(waited) = streams.get(stream) else { return };
        if waited.receiver_count() == 0 {
            streams.remove(stream);
            return;
        }
        waited.send_if_modified(|seen| {
            let later = revision > *seen;
            if later {
                *seen = revision;
            }
            later
        });
    }

    fn watch(&self, stream: &StreamKey) -> watch::Receiver<u64> {
        let mut streams = self.streams.lock().expect("stream waiters");
        if streams.len() >= KEPT {
            streams.retain(|_, waited| waited.receiver_count() > 0);
        }
        streams.entry(stream.clone()).or_insert_with(|| watch::channel(0).0).subscribe()
    }
}

impl Content {
    /// Waits until the stream's revision is past `after`, `within` at most,
    /// and answers the revision it has then; none while the stream is not here.
    pub async fn moved(&self, stream: &StreamKey, after: u64, within: Duration) -> Result<Option<u64>> {
        stream.check()?;
        // Watched before it is read, so a batch between the two is not missed.
        let mut watching = self.0.wakes.watch(stream);
        let key = stream.clone();
        let stored = self.0.database.read(move |db| crate::read::revision(db, &key)).await?;
        if stored.is_some_and(|revision| revision > after) {
            return Ok(stored);
        }
        let waited = tokio::time::timeout(within, async {
            loop {
                if watching.changed().await.is_err() {
                    return None;
                }
                let revision = *watching.borrow_and_update();
                if revision > after {
                    return Some(revision);
                }
            }
        })
        .await;
        Ok(waited.ok().flatten().or(stored))
    }
}
