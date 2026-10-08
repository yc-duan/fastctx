//! Watching residents live: every event of a session the World hears from,
//! handed to each watcher as it happens. A watcher that goes away is
//! dropped the next time there is something to hand it.

use std::collections::HashMap;
use std::sync::Mutex;

use fastctx_types::ResidentId;
use fastctx_types::session::SessionEvent;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct Watchers {
    watching: Mutex<HashMap<ResidentId, Watching>>,
}

#[derive(Default)]
struct Watching {
    senders: Vec<mpsc::UnboundedSender<SessionEvent>>,
    live: crate::session_live::Live,
}

impl Watchers {
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
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut watching = self.watching.lock().expect("never poisoned");
        let session = watching.entry(id.clone()).or_default();
        for snapshot in session.live.snapshots() {
            sender.send(snapshot.clone()).expect("receiver is still here");
        }
        session.senders.push(sender);
        receiver
    }

    pub fn publish(&self, id: &ResidentId, event: &SessionEvent) {
        let mut watching = self.watching.lock().expect("never poisoned");
        let session = watching.entry(id.clone()).or_default();
        if session.live.observe(event) {
            session.senders.retain(|watcher| watcher.send(event.clone()).is_ok());
        }
        if session.senders.is_empty() && session.live.snapshots().next().is_none() {
            watching.remove(id);
        }
    }
}
