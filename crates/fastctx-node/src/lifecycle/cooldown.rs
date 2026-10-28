//! A negative idle observation expires, or is invalidated by new activity.

use fastctx_types::ResidentId;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const NOT_IDLE: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub(crate) struct Cooldowns(Mutex<HashMap<ResidentId, (Instant, Arc<()>)>>);

impl Cooldowns {
    /// A token prevents a late probe from replacing activity observed while it ran.
    pub(super) fn begin(&self, id: &ResidentId) -> Option<Arc<()>> {
        let mut rows = self.0.lock().expect("never poisoned");
        let now = Instant::now();
        rows.retain(|_, (until, _)| *until > now);
        if rows.contains_key(id) {
            return None;
        }
        let token = Arc::new(());
        rows.insert(id.clone(), (now + NOT_IDLE, token.clone()));
        Some(token)
    }

    pub(super) fn clear(&self, id: &ResidentId, token: &Arc<()>) {
        let mut rows = self.0.lock().expect("never poisoned");
        if rows.get(id).is_some_and(|(_, current)| Arc::ptr_eq(current, token)) {
            rows.remove(id);
        }
    }

    pub(crate) fn touched(&self, id: &ResidentId) {
        self.0.lock().expect("never poisoned").remove(id);
    }
}
