//! Bounded waits distinguish a deadline from physical release of SQLite handles.
use super::Record;
use crate::{Error, ErrorKind, Result};
use std::{
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct MigrationGate(Arc<State>);
pub(super) struct WeakGate(Weak<State>);
struct State {
    startup: Mutex<Option<Result<()>>>,
    result: Mutex<Option<Result<Record>>>,
    changed: tokio::sync::Notify,
    blocking: Condvar,
    opened: Condvar,
    deadline: Instant,
    stop: Arc<AtomicBool>,
}
impl WeakGate {
    pub fn upgrade(&self) -> Option<MigrationGate> {
        self.0.upgrade().map(MigrationGate)
    }
}
impl std::fmt::Debug for MigrationGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MigrationGate").field("finished", &self.finished()).finish()
    }
}
impl MigrationGate {
    pub(super) fn new(elapsed: Duration) -> Self {
        Self(Arc::new(State {
            startup: Mutex::new(None),
            result: Mutex::new(None),
            changed: tokio::sync::Notify::new(),
            blocking: Condvar::new(),
            opened: Condvar::new(),
            deadline: Instant::now().checked_add(elapsed).unwrap_or_else(Instant::now),
            stop: Arc::new(AtomicBool::new(false)),
        }))
    }
    pub(super) fn downgrade(&self) -> WeakGate {
        WeakGate(Arc::downgrade(&self.0))
    }
    pub(super) fn deadline(&self) -> Instant {
        self.0.deadline
    }
    pub(super) fn stop_signal(&self) -> Arc<AtomicBool> {
        self.0.stop.clone()
    }
    pub(super) fn opened(&self, result: Result<()>) {
        *self.0.startup.lock().expect("upgrade startup") = Some(result);
        self.0.opened.notify_all();
    }
    pub(super) fn wait_opened(&self) -> Result<()> {
        let result = self.0.startup.lock().expect("upgrade startup");
        let (result, _) = self
            .0
            .opened
            .wait_timeout_while(result, Duration::from_secs(35), |r| r.is_none())
            .expect("upgrade startup");
        result.clone().unwrap_or_else(|| {
            Err(Error::with_kind(ErrorKind::Busy, "another opener is still pinning the format snapshot"))
        })
    }
    /// True only after the worker released its snapshot and returned. A timed-out
    /// waiter must not release bulk migration while this remains false.
    pub fn finished(&self) -> bool {
        self.0.result.lock().expect("upgrade completion").is_some()
    }
    pub(super) fn needs_resume(&self) -> bool {
        self.0.result.lock().expect("upgrade completion").as_ref().is_some_and(Result::is_err)
    }
    pub fn request_stop(&self) {
        self.0.stop.store(true, Ordering::Release);
    }
    /// A bounded lifecycle drain. On timeout the worker still owns its IO; callers
    /// must defer destructive filesystem work rather than report successful release.
    pub fn drain(&self, within: Duration) -> Result<()> {
        self.request_stop();
        let result = self.0.result.lock().expect("upgrade completion");
        let (result, _) =
            self.0.blocking.wait_timeout_while(result, within, |r| r.is_none()).expect("upgrade completion");
        if result.is_none() {
            return Err(Error::with_kind(
                ErrorKind::Busy,
                "format backup IO has not released; destructive cleanup deferred",
            ));
        }
        Ok(())
    }
    /// Returns by the configured backup deadline. Timeout does not mean SQLite's
    /// source handles were released: finished() remains false until that happens.
    pub fn wait_blocking(&self) -> Result<Record> {
        let result = self.0.result.lock().expect("upgrade completion");
        let remaining = self.0.deadline.saturating_duration_since(Instant::now());
        let (result, _) =
            self.0.blocking.wait_timeout_while(result, remaining, |r| r.is_none()).expect("upgrade completion");
        result.clone().unwrap_or_else(|| Err(wait_expired()))
    }
    pub async fn wait(&self) -> Result<Record> {
        tokio::time::timeout_at(tokio::time::Instant::from_std(self.0.deadline), self.released())
            .await
            .map_err(|_| wait_expired())?
    }
    /// Completion notification for event loops, not a blocking deadline wait. It
    /// tracks physical release after a timed-out wait without holding a database.
    pub async fn released(&self) -> Result<Record> {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(result) = self.0.result.lock().expect("upgrade completion").clone() {
                return result;
            }
            changed.await;
        }
    }
    pub(super) fn finish(&self, result: Result<Record>) {
        *self.0.result.lock().expect("upgrade completion") = Some(result);
        self.0.changed.notify_waiters();
        self.0.blocking.notify_all();
    }
}
fn wait_expired() -> Error {
    Error::with_kind(
        ErrorKind::Busy,
        "format backup wait exceeded its deadline; IO may still own the source snapshot and bulk migration remains paused",
    )
}
