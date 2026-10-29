//! Watch publication locks only this conversation's state. Small registry shards
//! locate the conversation and are released before observing or fanning out an
//! event. A slow consumer loses its bounded queue and resumes from history.

use fastctx_net::watch_queue::{self, Events, Sender};
use fastctx_types::ResidentId;
use fastctx_types::session::SessionEvent;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, RwLock};

const SHARDS: usize = 64;
type Sessions = HashMap<ResidentId, Arc<Mutex<Watching>>>;

pub struct Watchers {
    watching: Arc<[RwLock<Sessions>; SHARDS]>,
    pub(crate) changed: Arc<tokio::sync::Notify>,
}
impl Default for Watchers {
    fn default() -> Self {
        Self { watching: Arc::new(std::array::from_fn(|_| RwLock::default())), changed: Arc::default() }
    }
}
#[derive(Default)]
struct Watching {
    senders: Vec<Sender<Arc<SessionEvent>>>,
    live: crate::session_live::Live,
}
fn shard(id: &ResidentId) -> usize {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hash);
    hash.finish() as usize % SHARDS
}

impl Watchers {
    fn get(&self, id: &ResidentId) -> Option<Arc<Mutex<Watching>>> {
        self.watching[shard(id)].read().expect("never poisoned").get(id).cloned()
    }
    fn session(&self, id: &ResidentId) -> Arc<Mutex<Watching>> {
        self.get(id).unwrap_or_else(|| {
            self.watching[shard(id)].write().expect("never poisoned").entry(id.clone()).or_default().clone()
        })
    }
    pub(crate) fn watched(&self, id: &ResidentId) -> bool {
        let watched = self.get(id).is_some_and(|session| {
            session.lock().expect("never poisoned").senders.iter().any(|sender| !sender.is_closed())
        });
        retire(&self.watching, id);
        watched
    }
    pub fn tools(&self, id: &ResidentId) -> Vec<fastctx_types::town::CurrentTool> {
        use fastctx_types::session::Happened;
        let Some(session) = self.get(id) else { return Vec::new() };
        let tools = session
            .lock()
            .expect("never poisoned")
            .live
            .snapshots()
            .filter_map(|event| {
                let Happened::Began { turn, item } = &event.happened else { return None };
                fastctx_types::town::CurrentTool::from_item(turn, item)
            })
            .collect();
        drop(session);
        retire(&self.watching, id);
        tools
    }
    pub async fn unwatched(&self, id: &ResidentId) {
        loop {
            let sender = self.get(id).and_then(|session| {
                session.lock().expect("never poisoned").senders.iter().find(|sender| !sender.is_closed()).cloned()
            });
            retire(&self.watching, id);
            match sender {
                Some(sender) => sender.closed().await,
                None => return,
            }
        }
    }
    pub fn watch(&self, id: &ResidentId) -> Events {
        self.subscribe(id, false)
    }
    pub fn watch_composer(&self, id: &ResidentId) -> Events {
        self.subscribe(id, true)
    }

    fn subscribe(&self, id: &ResidentId, composer: bool) -> Events {
        let (sender, receiver) = watch_queue::events();
        let cell = self.session(id);
        let mut session = cell.lock().expect("never poisoned");
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
            if sender.send(Arc::new(snapshot.clone())).is_err() {
                break;
            }
        }
        session.senders.push(sender.clone());
        drop(session);
        self.changed.notify_one();
        let changed = self.changed.clone();
        let (registry, id) = (Arc::downgrade(&self.watching), id.clone());
        tokio::spawn(async move {
            sender.closed().await;
            changed.notify_one();
            {
                let mut session = cell.lock().expect("never poisoned");
                session.senders.retain(|held| !held.same_channel(&sender));
                if session.senders.is_empty() {
                    session.live.unwatched();
                }
            }
            drop(cell);
            if let Some(registry) = registry.upgrade() {
                retire(&registry, &id);
            }
        });
        receiver
    }

    pub fn publish(&self, id: &ResidentId, event: &SessionEvent) {
        let cell = self.session(id);
        let measured = watch_queue::event_size(event);
        let empty = {
            let mut session = cell.lock().expect("never poisoned");
            if session.live.observe(event) && !session.senders.is_empty() {
                let event = Arc::new(event.clone());
                session
                    .senders
                    .retain(|sender| sender.send_measured(event.clone(), measured.0, measured.1.clone()).is_ok());
            }
            if session.senders.is_empty() {
                session.live.unwatched();
            }
            session.senders.is_empty() && session.live.snapshots().next().is_none()
        };
        drop(cell);
        if empty {
            retire(&self.watching, id);
        }
    }
}

fn retire(registry: &[RwLock<Sessions>; SHARDS], id: &ResidentId) {
    // Callers release their cell before checking. The last concurrent user
    // therefore sees the registry's sole reference, even when several close
    // together. Holding the shard's write lock prevents a new lookup racing it.
    let mut shard = registry[shard(id)].write().expect("never poisoned");
    let empty = shard.get(id).is_some_and(|cell| {
        if Arc::strong_count(cell) != 1 {
            return false;
        }
        let session = cell.lock().expect("never poisoned");
        session.senders.is_empty() && session.live.snapshots().next().is_none()
    });
    if empty {
        shard.remove(id);
    }
}
