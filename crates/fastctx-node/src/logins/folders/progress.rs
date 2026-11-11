//! Coalesce a walk's intermediate reports, and always publish its last state.
use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) struct Progress {
    topics: Arc<crate::topics::Topics>,
    last: Cell<Instant>,
}

impl Progress {
    pub(super) fn new(topics: Arc<crate::topics::Topics>) -> Self {
        Self { topics, last: Cell::new(Instant::now()) }
    }

    pub(super) fn changed(&self) {
        let now = Instant::now();
        if now.duration_since(self.last.get()) >= Duration::from_millis(200) {
            self.last.set(now);
            self.topics.changed(fastctx_types::topics::LOGINS);
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.topics.changed(fastctx_types::topics::LOGINS);
    }
}
