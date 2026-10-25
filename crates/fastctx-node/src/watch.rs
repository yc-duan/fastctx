//! Watching residents live: every event of a session the World hears from,
//! handed to each watcher as it happens. Closing a receiver releases its
//! registration even when the session produces no further events.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fastctx_types::ResidentId;
use fastctx_types::session::SessionEvent;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct Watchers {
    watching: Arc<Mutex<HashMap<ResidentId, Watching>>>,
    pub(crate) changed: Arc<tokio::sync::Notify>,
}

#[derive(Default)]
struct Watching {
    senders: Vec<mpsc::UnboundedSender<SessionEvent>>,
    live: crate::session_live::Live,
}

impl Watchers {
    pub(crate) fn watched(&self, id: &ResidentId) -> bool {
        self.watching
            .lock()
            .expect("never poisoned")
            .get(id)
            .is_some_and(|session| session.senders.iter().any(|sender| !sender.is_closed()))
    }

    /// A projection of the same in-flight items delivered to a joining watcher.
    pub fn tools(&self, id: &ResidentId) -> Vec<fastctx_types::town::CurrentTool> {
        use fastctx_types::session::Happened;
        let watching = self.watching.lock().expect("never poisoned");
        watching
            .get(id)
            .into_iter()
            .flat_map(|session| session.live.snapshots())
            .filter_map(|event| {
                let Happened::Began { turn, item } = &event.happened else { return None };
                fastctx_types::town::CurrentTool::from_item(turn, item)
            })
            .collect()
    }

    /// Ends as soon as no current watcher remains. Newly opened watches
    /// participate on the next check; no polling lease can keep one alive.
    pub async fn unwatched(&self, id: &ResidentId) {
        loop {
            let sender = self
                .watching
                .lock()
                .expect("never poisoned")
                .get(id)
                .and_then(|watching| watching.senders.iter().find(|sender| !sender.is_closed()).cloned());
            match sender {
                Some(sender) => sender.closed().await,
                None => return,
            }
        }
    }

    /// Everything that happens in the resident's session from now on.
    pub fn watch(&self, id: &ResidentId) -> mpsc::UnboundedReceiver<SessionEvent> {
        self.subscribe(id, false)
    }

    pub fn watch_composer(&self, id: &ResidentId) -> mpsc::UnboundedReceiver<SessionEvent> {
        self.subscribe(id, true)
    }

    fn subscribe(&self, id: &ResidentId, composer: bool) -> mpsc::UnboundedReceiver<SessionEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut watching = self.watching.lock().expect("never poisoned");
        let session = watching.entry(id.clone()).or_default();
        for snapshot in session.live.snapshots() {
            if composer
                && matches!(
                    &snapshot.happened,
                    fastctx_types::session::Happened::Changed {
                        change: fastctx_types::session::SessionChange::Composer { .. },
                        ..
                    }
                )
            {
                continue;
            }
            sender.send(snapshot.clone()).expect("receiver is still here");
        }
        session.senders.push(sender.clone());
        self.changed.notify_one();
        let changed = self.changed.clone();
        let (registry, id) = (Arc::downgrade(&self.watching), id.clone());
        tokio::spawn(async move {
            sender.closed().await;
            changed.notify_one();
            if let Some(registry) = registry.upgrade() {
                let mut watching = registry.lock().expect("never poisoned");
                if let Some(session) = watching.get_mut(&id) {
                    session.senders.retain(|held| !held.same_channel(&sender));
                    if session.senders.is_empty() {
                        session.live.unwatched();
                        if session.live.snapshots().next().is_none() {
                            watching.remove(&id);
                        }
                    }
                }
                if watching.is_empty() {
                    *watching = HashMap::new();
                }
            }
        });
        receiver
    }

    pub fn publish(&self, id: &ResidentId, event: &SessionEvent) {
        let mut watching = self.watching.lock().expect("never poisoned");
        let session = watching.entry(id.clone()).or_default();
        if session.live.observe(event) {
            session.senders.retain(|watcher| watcher.send(event.clone()).is_ok());
        }
        if session.senders.is_empty() {
            session.live.unwatched();
        }
        if session.senders.is_empty() && session.live.snapshots().next().is_none() {
            watching.remove(id);
        }
    }
}
