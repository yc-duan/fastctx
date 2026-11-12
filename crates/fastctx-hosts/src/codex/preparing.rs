//! Threads being made ready for words now, and how far that came: what a
//! view shows while a Codex conversation starts (`KeptWords::starting`).

use super::CodexHost;
use fastctx_types::session::StartPhase;
use jiff::Timestamp;

/// A thread being made ready, from when the first of those asking began;
/// how many ask.
pub(super) struct Making {
    phase: StartPhase,
    since: Timestamp,
    asking: usize,
}

/// One asker's part in making a thread ready; it ends when this drops.
pub(super) struct Preparing<'a> {
    host: &'a CodexHost,
    thread: String,
}

impl CodexHost {
    /// Marks `thread` as being loaded (its app-server started, the thread
    /// resumed) until the returned guard drops.
    pub(super) fn preparing(&self, thread: &str) -> Preparing<'_> {
        let mut state = self.lock();
        let making = state.preparing.entry(thread.to_string()).or_insert(Making {
            phase: StartPhase::Launching,
            since: Timestamp::now(),
            asking: 0,
        });
        making.asking += 1;
        Preparing { host: self, thread: thread.to_string() }
    }

    /// How far making `thread` ready has come, and since when, while it is
    /// being made ready.
    pub fn starting(&self, thread: &str) -> Option<(StartPhase, Timestamp)> {
        self.lock().preparing.get(thread).map(|making| (making.phase, making.since))
    }
}

impl Preparing<'_> {
    /// The thread is loaded; its World tools are not yet started.
    pub fn connecting(&self) {
        if let Some(making) = self.host.lock().preparing.get_mut(&self.thread) {
            making.phase = StartPhase::Connecting;
        }
    }
}

impl Drop for Preparing<'_> {
    fn drop(&mut self) {
        let mut state = self.host.lock();
        if let Some(making) = state.preparing.get_mut(&self.thread) {
            making.asking -= 1;
            if making.asking == 0 {
                state.preparing.remove(&self.thread);
            }
        }
    }
}
