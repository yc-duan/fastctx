//! Live timer identities are shared with their short delivery handles.

use fastctx_types::{NoticeId, timers::Timer};
use jiff::Timestamp;
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

#[derive(Default, Clone)]
pub(super) struct Claims {
    pub active: HashMap<Arc<str>, NoticeId>,
    pub delayed: HashMap<Arc<str>, HashMap<NoticeId, Timestamp>>,
    handles: HashMap<Arc<str>, BTreeSet<NoticeId>>,
    identities: HashMap<NoticeId, Arc<str>>,
}

impl Claims {
    fn key(&self, id: &str) -> Arc<str> {
        self.active
            .get_key_value(id)
            .map(|(key, _)| key)
            .or_else(|| self.delayed.get_key_value(id).map(|(key, _)| key))
            .or_else(|| self.handles.get_key_value(id).map(|(key, _)| key))
            .cloned()
            .unwrap_or_else(|| Arc::from(id))
    }
    pub fn alias(&mut self, id: &str, delivery: &NoticeId) -> Arc<str> {
        let key = self.key(id);
        self.handles.entry(key.clone()).or_default().insert(delivery.clone());
        self.identities.insert(delivery.clone(), key.clone());
        key
    }
    fn clean(&mut self, id: &str) {
        if !self.active.contains_key(id)
            && !self.delayed.contains_key(id)
            && let Some(handles) = self.handles.remove(id)
        {
            for handle in handles {
                self.identities.remove(&handle);
            }
        }
    }
    pub fn settled(&mut self, id: &str) {
        self.delayed.remove(id);
        self.clean(id);
    }
    /// Keeps only the live firing, backoffs and the newest setting's handle.
    pub fn replaced(&mut self, id: &str, delivery: &NoticeId) {
        if !self.active.contains_key(id) {
            return;
        }
        let active = self.active.get(id);
        let delayed = self.delayed.get(id);
        if let Some(handles) = self.handles.get_mut(id) {
            handles.retain(|handle| {
                let keep = Some(handle) == active || delayed.is_some_and(|values| values.contains_key(handle));
                if !keep {
                    self.identities.remove(handle);
                }
                keep
            });
        }
        self.alias(id, delivery);
    }
    pub fn settled_handle(&mut self, handle: &NoticeId, replacement: Option<&NoticeId>) {
        if let Some(key) = self.identities.get(handle).cloned() {
            self.settled(&key);
            if let Some(delivery) = replacement {
                self.replaced(&key, delivery);
            }
        }
    }
    pub fn unclaim(&mut self, timer: &Timer) {
        if self.active.get(timer.id.as_str()) == Some(&timer.delivery) {
            self.active.remove(timer.id.as_str());
        }
        self.clean(&timer.id);
    }
}
