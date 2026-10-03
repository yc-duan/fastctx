//! Transient subscription lifecycle. Neither events nor subscriptions survive a link loss.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use tokio::sync::Notify;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Message {
    pub(crate) id: String,
    pub(crate) value: serde_json::Value,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Notice {
    Started {
        id: String,
        from: String,
        topic: serde_json::Value,
    },
    Event {
        id: String,
        from: String,
        value: serde_json::Value,
    },
    Closed {
        id: String,
        peer: String,
        reason: String,
    },
}
#[derive(Clone)]
pub(crate) struct Active {
    pub(crate) peer: String,
    pub(crate) publishing: bool,
}
#[derive(Default)]
pub(crate) struct Subscriptions {
    pub(crate) active: Mutex<BTreeMap<String, Active>>,
    queue: Mutex<VecDeque<Notice>>,
    changed: Notify,
}
impl Subscriptions {
    pub(crate) fn push(&self, notice: Notice) {
        self.queue.lock().push_back(notice);
        self.changed.notify_one();
    }
    pub(crate) async fn next(&self) -> Notice {
        loop {
            let notified = self.changed.notified();
            if let Some(notice) = self.queue.lock().pop_front() {
                return notice;
            }
            notified.await;
        }
    }
    pub(crate) fn close(&self, id: &str, reason: &str) {
        if let Some(active) = self.active.lock().remove(id) {
            self.push(Notice::Closed {
                id: id.into(),
                peer: active.peer,
                reason: reason.into(),
            });
        }
    }
    pub(crate) fn disconnect(&self) {
        // A peer may close just before our own link disappears. Preserve that notification
        // even though its active entry was already removed; events themselves are transient.
        self.queue
            .lock()
            .retain(|notice| matches!(notice, Notice::Closed { .. }));
        let entries = std::mem::take(&mut *self.active.lock());
        for (id, active) in entries {
            self.push(Notice::Closed {
                id,
                peer: active.peer,
                reason: "local link disconnected".into(),
            });
        }
    }
}
