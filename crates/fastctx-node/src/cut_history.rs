//! The segments a conversation's history was last cut into for its viewers,
//! kept a while (`held_history`): one that comes next is sent them and the
//! events told since they were cut, and its history is not read again. A
//! cut goes once its events are no longer all kept (`watch::Watchers`), or
//! the history changed after it, or the cuts kept together grow past their
//! bound, the one least lately sent first.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fastctx_net::history::Cut;
use fastctx_types::ResidentId;
use fastctx_types::session::Mark;

/// The bytes the cuts of every conversation may hold together.
const KEPT_BYTES: usize = 128 << 20;

/// One conversation's history as it was cut.
pub(crate) struct CutHistory {
    /// The mark of the last event told before the history was read: the
    /// events after it follow the cut.
    pub at: Mark,
    pub version: String,
    pub cut: Cut,
    /// What was under way as the events after `at` began.
    pub snapshots: Vec<Arc<crate::watch::Marked>>,
    bytes: usize,
}

impl CutHistory {
    pub fn new(at: Mark, version: String, cut: Cut, snapshots: Vec<Arc<crate::watch::Marked>>) -> Self {
        let bytes = cut.sizes.iter().sum::<usize>()
            + snapshots.iter().map(|marked| fastctx_net::watch_queue::event_size(&marked.event).0).sum::<usize>();
        Self { at, version, cut, snapshots, bytes }
    }
}

#[derive(Default)]
pub(crate) struct CutHistories(Mutex<Kept>);

#[derive(Default)]
struct Kept {
    cuts: HashMap<ResidentId, (Arc<CutHistory>, u64)>,
    bytes: usize,
    sent: u64,
}

impl CutHistories {
    /// The cut kept of `id`'s history, now sent once more.
    pub fn get(&self, id: &ResidentId) -> Option<Arc<CutHistory>> {
        let mut kept = self.0.lock().expect("never poisoned");
        kept.sent += 1;
        let sent = kept.sent;
        let (cut, used) = kept.cuts.get_mut(id)?;
        *used = sent;
        Some(cut.clone())
    }

    /// Keeps `cut` as `id`'s, in place of any before it, letting the least
    /// lately sent go past the bound. One larger than the bound is not kept.
    pub fn keep(&self, id: &ResidentId, cut: CutHistory) {
        let mut kept = self.0.lock().expect("never poisoned");
        if let Some((gone, _)) = kept.cuts.remove(id) {
            kept.bytes -= gone.bytes;
        }
        if cut.bytes > KEPT_BYTES {
            return;
        }
        while kept.bytes + cut.bytes > KEPT_BYTES {
            let Some(oldest) = kept.cuts.iter().min_by_key(|(_, (_, used))| *used).map(|(id, _)| id.clone()) else {
                break;
            };
            if let Some((gone, _)) = kept.cuts.remove(&oldest) {
                kept.bytes -= gone.bytes;
            }
        }
        kept.sent += 1;
        let sent = kept.sent;
        kept.bytes += cut.bytes;
        kept.cuts.insert(id.clone(), (Arc::new(cut), sent));
    }

    /// Lets `id`'s cut go, when it is still `cut`.
    pub fn forget(&self, id: &ResidentId, cut: &Arc<CutHistory>) {
        let mut kept = self.0.lock().expect("never poisoned");
        if kept.cuts.get(id).is_some_and(|(current, _)| Arc::ptr_eq(current, cut))
            && let Some((gone, _)) = kept.cuts.remove(id)
        {
            kept.bytes -= gone.bytes;
        }
    }
}
