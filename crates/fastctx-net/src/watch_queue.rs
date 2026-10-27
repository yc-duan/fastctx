//! Observation queues throughout the node/client watch path share a byte budget.
//! Only complete composer and conversation state snapshots replace one another;
//! transcript events and document deltas remain ordered and are never discarded
//! without making the consumer resume its watch.

use fastctx_platform::event_queue::{self, Budget};
use fastctx_types::session::{Happened, SessionChange, SessionEvent, Watched};
use serde::Serialize;
use std::sync::{Arc, LazyLock};

pub use event_queue::{Lagged, Receiver, Sender, TryRecvError};
pub type Events = Receiver<Arc<SessionEvent>>;
static MEMORY: LazyLock<Budget> = LazyLock::new(|| Budget::new(128 << 20));

pub fn channel<T>(measure: fn(&T) -> (usize, Option<String>)) -> (Sender<T>, Receiver<T>) {
    event_queue::channel(256, 4 << 20, 64 << 20, MEMORY.clone(), measure)
}

pub fn events() -> (Sender<Arc<SessionEvent>>, Events) {
    channel(|event| event_size(event))
}

pub fn event_size(event: &SessionEvent) -> (usize, Option<String>) {
    let key = match &event.happened {
        Happened::Changed { change: SessionChange::Composer { .. }, .. } => Some("composer".into()),
        Happened::Changed { change: SessionChange::Goal { goal }, .. } if goal["world"] == true => Some("goal".into()),
        _ => None,
    };
    (size(event), key)
}

pub fn watched_size(watched: &Watched) -> (usize, Option<String>) {
    match watched {
        Watched::Now { .. } => (size(watched), Some("now".into())),
        Watched::Live { event } => event_size(event),
        _ => (size(watched), None),
    }
}

/// Counts encoded bytes without allocating a second copy of a large history.
pub fn size(value: &impl Serialize) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    if serde_json::to_writer(&mut count, value).is_err() {
        return usize::MAX;
    }
    count.0
}

pub fn queued_bytes() -> usize {
    MEMORY.used()
}
