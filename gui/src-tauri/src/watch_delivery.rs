//! The WebView receives at most one pending wakeup, never an unbounded sequence
//! of payloads. It pulls bounded UTF-8 chunks and acknowledges each before the
//! next. A queue gap drops its source and resumes only when the page asks again.

mod mailbox;
pub(crate) use mailbox::{Mailbox, Read, Sender};

use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;

#[derive(Clone, Serialize)]
pub(crate) struct Wake {
    id: u64,
}

#[derive(Default)]
pub(crate) struct Deliveries(Mutex<HashMap<u64, Arc<Mailbox>>>);
impl Deliveries {
    pub fn open(&self, id: u64, wake: Channel<Wake>) -> Result<Arc<Mailbox>, String> {
        let mut entries = self.0.lock().expect("WebView deliveries");
        if entries.len() >= 1024 {
            return Err("this page has too many open observation streams".into());
        }
        let mailbox = Mailbox::new(id, wake);
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
    fn read(&self, id: u64, through: u64, resync: bool) -> Result<Read, String> {
        let mailbox = self.0.lock().expect("WebView deliveries").get(&id).cloned();
        let Some(mailbox) = mailbox else {
            return Ok(Read::Closed);
        };
        let read = mailbox.read(through, resync)?;
        if matches!(read, Read::Closed) {
            self.close(id);
        }
        Ok(read)
    }
}

#[tauri::command]
pub(crate) fn watch_delivery_read(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    through: u64,
    resync: Option<bool>,
) -> Result<Read, String> {
    let read = shell.deliveries.read(id, through, resync.unwrap_or(false))?;
    if matches!(read, Read::Closed) {
        shell.transcripts.close(id);
    }
    Ok(read)
}

impl crate::Shell {
    /// A stream owns its source only while the page can consume it. After a gap,
    /// the factory starts afresh under the same public id and sends a snapshot.
    pub(crate) fn follow_delivery<T, F, Fut>(
        &self,
        page: u64,
        id: u64,
        wake: Channel<Wake>,
        key: fn(&T) -> Option<String>,
        mut work: F,
    ) -> Result<(), String>
    where
        T: Serialize + Send + 'static,
        F: FnMut(Sender<T>, bool) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let mailbox = self.deliveries.open(id, wake)?;
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
