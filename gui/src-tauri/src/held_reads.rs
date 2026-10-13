//! The file reads a page holds. A read goes on through breaks while it is
//! wanted (`fastctx_client::file_read`), so the page names each read it may
//! give up on: it is let go when the page says so (`cancel_read`), or when
//! the page closes. How each goes is told down the page's channel.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fastctx_client::file_read::Progress;
use serde::Serialize;
use tauri::ipc::{Channel, JavaScriptChannelId};
use tokio::sync::{oneshot, watch};

/// How a read goes, as the page is told it.
#[derive(Clone, Serialize)]
pub(crate) struct ReadProgress {
    received: u64,
    /// The bytes asked of the file, once it opened.
    length: Option<u64>,
    /// While the read is broken off: why, and since when (milliseconds
    /// since the Unix epoch).
    interrupted: Option<ReadInterrupted>,
}

#[derive(Clone, Serialize)]
struct ReadInterrupted {
    why: String,
    since: u64,
}

impl From<&Progress> for ReadProgress {
    fn from(progress: &Progress) -> Self {
        Self {
            received: progress.received,
            length: progress.length,
            interrupted: progress.interrupted.as_ref().map(|interruption| ReadInterrupted {
                why: interruption.why.clone(),
                since: crate::epoch_ms(interruption.since),
            }),
        }
    }
}

/// How often the page is told at most: often enough that a reader sees
/// the count move, and no oftener than a tenth of a second, as fast as one
/// reads it.
const PACE: Duration = Duration::from_millis(100);

/// The channel a page gave a read, if it gave one: an optional command
/// argument names it, which Tauri takes only as its id.
pub(crate) fn channel(webview: &tauri::Webview, id: Option<JavaScriptChannelId>) -> Option<Channel<ReadProgress>> {
    id.map(|id| id.channel_on(webview.clone()))
}

/// Tells `channel`, when the page gave one, how the read behind `progress`
/// goes, until the read is dropped or the page stops listening.
pub(crate) fn tell(mut progress: watch::Receiver<Progress>, channel: Option<Channel<ReadProgress>>) {
    let Some(channel) = channel else { return };
    tauri::async_runtime::spawn(async move {
        loop {
            let said = ReadProgress::from(&*progress.borrow_and_update());
            if channel.send(said).is_err() {
                return;
            }
            tokio::time::sleep(PACE).await;
            if progress.changed().await.is_err() {
                return;
            }
        }
    });
}

/// The reads the page holds, each under the id it gave.
#[derive(Default)]
pub(crate) struct HeldReads {
    held: Mutex<HashMap<String, (u64, oneshot::Sender<()>)>>,
    next: AtomicU64,
}

impl HeldReads {
    /// Runs `work`, the read the page named `id`, until it is done or let
    /// go; `None` when it was let go. A read the page gave no id is held
    /// until it is done. The same id given again lets go of the earlier.
    pub(crate) async fn hold<T>(&self, id: Option<String>, work: impl Future<Output = T>) -> Option<T> {
        let Some(id) = id else { return Some(work.await) };
        let (holding, let_go) = oneshot::channel();
        let token = self.next.fetch_add(1, Ordering::Relaxed);
        self.held.lock().expect("held reads").insert(id.clone(), (token, holding));
        let _held = Held { reads: self, id, token };
        tokio::select! {
            done = work => Some(done),
            _ = let_go => None,
        }
    }

    /// Lets go of the read `id`, if it is still held.
    pub(crate) fn release(&self, id: &str) {
        self.held.lock().expect("held reads").remove(id);
    }

    /// Lets go of every read the page holds: it closed.
    pub(crate) fn release_all(&self) {
        self.held.lock().expect("held reads").clear();
    }
}

/// A read held under its id until it ends, however it ends.
struct Held<'a> {
    reads: &'a HeldReads,
    id: String,
    token: u64,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        let mut held = self.reads.held.lock().expect("held reads");
        if held.get(&self.id).is_some_and(|(token, _)| *token == self.token) {
            held.remove(&self.id);
        }
    }
}

/// What a command answers when the page let go of its read.
pub(crate) const LET_GO: &str = "the page let go of this read";
