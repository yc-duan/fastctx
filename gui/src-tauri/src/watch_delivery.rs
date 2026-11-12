//! The shell pushes each page stream's pieces over its Channel in numbered
//! batches, holding at most a window the page has not acknowledged; the page
//! acknowledges each batch once it ran, without waiting for an answer. The
//! queue behind it stays bounded (`watch_queue`): a page that falls behind is
//! pushed a gap, and the source starts again from what is now once the page
//! acknowledged it.

mod mailbox;
pub(crate) use mailbox::{Mailbox, Sender};

use serde::Serialize;
use serde_json::value::RawValue;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;

/// What a page stream's Channel carries.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Push {
    /// The next batch of pieces, each the JSON its source sent.
    Pieces { id: u64, number: u64, pieces: Vec<Box<RawValue>> },
    /// What waited was dropped: the page shows it fell behind and the source
    /// starts again once the page acknowledged this.
    Gap { id: u64, number: u64 },
    /// The stream is over.
    Closed { id: u64 },
}

#[derive(Default)]
pub(crate) struct Deliveries(Mutex<HashMap<u64, Arc<Mailbox>>>);
impl Deliveries {
    pub fn open(&self, id: u64, channel: Channel<Push>) -> Result<Arc<Mailbox>, String> {
        let mut entries = self.0.lock().expect("WebView deliveries");
        // What the streams hold is bounded on its own (`watch_queue`); this
        // only stops a page gone wrong.
        if entries.len() >= 16_384 {
            return Err("this page has too many open observation streams".into());
        }
        let mailbox = Mailbox::new(id, channel);
        entries.insert(id, mailbox.clone());
        Ok(mailbox)
    }
    pub fn close(&self, id: u64) {
        if let Some(mailbox) = self.0.lock().expect("WebView deliveries").remove(&id) {
            mailbox.close();
        }
    }
    pub fn close_all(&self) {
        for (_, mailbox) in self.0.lock().expect("WebView deliveries").drain() {
            mailbox.close();
        }
    }
    fn get(&self, id: u64) -> Option<Arc<Mailbox>> {
        self.0.lock().expect("WebView deliveries").get(&id).cloned()
    }
}

/// The page ran a stream's batches up to `through`; a stream whose close it
/// took lets go of what reads it.
#[tauri::command]
pub(crate) async fn watch_delivery_ack(shell: tauri::State<'_, crate::Shell>, id: u64, through: u64) -> Result<(), String> {
    let Some(mailbox) = shell.deliveries.get(id) else { return Ok(()) };
    if mailbox.acknowledge(through)? {
        shell.deliveries.close(id);
        shell.transcripts.close(id);
        shell.timeline_readers.close(id);
    }
    Ok(())
}

/// The page missed a stream's batches after `through`: they go again.
#[tauri::command]
pub(crate) async fn watch_delivery_resend(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    through: u64,
) -> Result<(), String> {
    match shell.deliveries.get(id) {
        Some(mailbox) => mailbox.resend(through),
        None => Ok(()),
    }
}

impl crate::Shell {
    /// A stream owns its source only while the page can consume it. After a gap,
    /// the factory starts afresh under the same public id and sends a snapshot.
    pub(crate) fn follow_delivery<T, F, Fut>(
        &self,
        page: u64,
        id: u64,
        pieces: Channel<Push>,
        key: fn(&T) -> Option<String>,
        mut work: F,
    ) -> Result<(), String>
    where
        T: Serialize + Send + 'static,
        F: FnMut(Sender<T>, bool) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let mailbox = self.deliveries.open(id, pieces)?;
        let result = self.follow(page, id, async move {
            let mut restarts = mailbox.restarts();
            let mut again = false;
            loop {
                let generation = *restarts.borrow_and_update();
                let Some(sender) = mailbox.sender(generation, key) else {
                    return;
                };
                let failed = sender.failed.clone();
                tokio::select! {
                    () = work(sender, again) => {
                        if *restarts.borrow() != generation { again = true; continue; }
                        if !failed.load(std::sync::atomic::Ordering::Acquire) {
                            mailbox.finish(generation);
                            return;
                        }
                    }
                    result = restarts.changed() => {
                        if result.is_err() || mailbox.closed() { return; }
                        again = true;
                        continue;
                    }
                }
                // This is an idle observation, with no source or queued work kept.
                // Closing the page aborts it; consuming the gap starts a new source.
                if restarts.changed().await.is_err() || mailbox.closed() {
                    return;
                }
                again = true;
            }
        });
        if result.is_err() {
            self.deliveries.close(id);
        }
        result
    }
}
