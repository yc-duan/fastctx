//! Stops asked for while words are on their way into a session, before the
//! turn they start is known (`world-gui.md`, 停止键). The stop is kept with
//! those words, and the turn they start is stopped as soon as it is known.
//! Words that start no turn take their stop with them: their ids are never
//! bound to another turn, so a stop kept here never stops a later one.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use fastctx_types::{NoticeId, ResidentId};

use crate::Node;

#[derive(Default)]
pub(crate) struct Stops {
    /// The words of each resident whose turn is to stop once it is known.
    wanted: Mutex<HashMap<ResidentId, HashSet<NoticeId>>>,
}

impl Stops {
    /// A stop for `resident`, which runs no turn the World knows of: the
    /// words on their way into its session, `coming`, keep it. Whether any
    /// did; none when a turn is known by now, which is stopped as any is.
    /// Checked against the resident's turn under the lock its words' turn is
    /// told through (`started`), so neither misses the other.
    pub(crate) fn coming(&self, node: &Node, resident: &ResidentId, coming: Vec<NoticeId>) -> bool {
        let mut wanted = self.wanted.lock().expect("never poisoned");
        if coming.is_empty() || node.residents.now(resident).is_some_and(|now| now.turn.is_some()) {
            return false;
        }
        let kept = wanted.entry(resident.clone()).or_default();
        // Only words still on their way keep a stop.
        kept.retain(|id| coming.contains(id));
        kept.extend(coming);
        true
    }

    /// The turn of `resident` that took `words` is known and entered in its
    /// state: whether a stop waits for it.
    pub(crate) fn started<'a>(&self, resident: &ResidentId, words: impl IntoIterator<Item = &'a NoticeId>) -> bool {
        let mut wanted = self.wanted.lock().expect("never poisoned");
        let Some(kept) = wanted.get_mut(resident) else { return false };
        let mut stopped = false;
        for id in words {
            stopped |= kept.remove(id);
        }
        if kept.is_empty() {
            wanted.remove(resident);
        }
        stopped
    }
}
