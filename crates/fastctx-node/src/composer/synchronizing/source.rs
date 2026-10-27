//! Ordered, bounded document publication. Commit callbacks only enqueue measured
//! packets; each conversation fans them out on its own task after commit.

use super::*;
use std::sync::Weak;

type Consumers = Arc<Mutex<HashMap<String, watch_queue::Sender<Arc<Frame>>>>>;
type Entries = Arc<Mutex<HashMap<ResidentId, Weak<Shared>>>>;
#[derive(Default)]
pub(in crate::composer) struct Sources(Entries);

pub(in crate::composer) struct Shared {
    id: ResidentId,
    registry: Weak<Mutex<HashMap<ResidentId, Weak<Shared>>>>,
    consumers: Consumers,
    publish: watch_queue::Sender<Measured>,
}
struct Measured {
    frame: Arc<Frame>,
    bytes: usize,
}
pub(in crate::composer) struct Publication {
    registry: Weak<Mutex<HashMap<ResidentId, Weak<Shared>>>>,
    id: ResidentId,
    measured: Measured,
}
impl Publication {
    pub fn committed(self) {
        if let Some(registry) = self.registry.upgrade() {
            let source = registry.lock().expect("document streams").get(&self.id).cloned();
            if let Some(source) = source.and_then(|source| source.upgrade()) {
                let _ = source.publish.send(self.measured);
            }
        }
    }
}

impl Sources {
    pub fn prepare(&self, id: &ResidentId, frame: Frame) -> Publication {
        let bytes = watch_queue::size(&frame);
        Publication {
            registry: Arc::downgrade(&self.0),
            id: id.clone(),
            measured: Measured { frame: Arc::new(frame), bytes },
        }
    }
    pub(super) fn open(&self, id: &ResidentId) -> Arc<Shared> {
        let mut entries = self.0.lock().expect("document streams");
        let previous = entries.get(id).and_then(Weak::upgrade);
        if previous.as_ref().is_some_and(|shared| !shared.publish.is_closed()) {
            return previous.expect("open source");
        }
        let (publish, mut incoming) = watch_queue::channel(|packet: &Measured| (packet.bytes, None));
        let consumers: Consumers = Arc::default();
        let shared = Arc::new(Shared {
            id: id.clone(),
            registry: Arc::downgrade(&self.0),
            consumers: consumers.clone(),
            publish,
        });
        entries.insert(id.clone(), Arc::downgrade(&shared));
        drop(entries);
        drop(previous);
        tokio::spawn(async move {
            loop {
                match incoming.recv().await {
                    Ok(Some(packet)) => {
                        consumers.lock().expect("document stream consumers").retain(|_, consumer| {
                            consumer.send_measured(packet.frame.clone(), packet.bytes, None).is_ok()
                        });
                    }
                    Ok(None) => break,
                    Err(_) => {
                        for consumer in consumers.lock().expect("document stream consumers").values() {
                            consumer.lag();
                        }
                        break;
                    }
                }
            }
        });
        shared
    }
}
impl Shared {
    pub(super) fn subscribe(&self, name: &str) -> watch_queue::Receiver<Arc<Frame>> {
        let (sender, receiver) = watch_queue::channel(|frame: &Arc<Frame>| (watch_queue::size(frame.as_ref()), None));
        self.consumers.lock().expect("document stream consumers").insert(name.into(), sender);
        receiver
    }
    pub(super) fn remove(&self, name: &str) {
        self.consumers.lock().expect("document stream consumers").remove(name);
    }
}
impl Drop for Shared {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            let mut registry = registry.lock().expect("document streams");
            if registry.get(&self.id).is_some_and(|shared| std::ptr::eq(shared.as_ptr(), self)) {
                registry.remove(&self.id);
            }
        }
    }
}
