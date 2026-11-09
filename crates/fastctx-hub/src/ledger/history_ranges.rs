//! Append-only ranges let projections skip history without copying its sequence index.

use std::{collections::BTreeMap, sync::RwLock};

#[derive(Default)]
pub(super) struct HistoryRanges(RwLock<BTreeMap<u64, u64>>);

impl HistoryRanges {
    pub fn append(&self, seq: u64) {
        let mut ranges = self.0.write().expect("never poisoned");
        if let Some(mut last) = ranges.last_entry()
            && last.get().checked_add(1) == Some(seq)
        {
            *last.get_mut() = seq;
            return;
        }
        ranges.insert(seq, seq);
    }

    /// Appends can extend a range while a page reads; its committed end cannot move.
    pub fn through(&self, seq: u64, last: u64) -> Option<u64> {
        let ranges = self.0.read().expect("never poisoned");
        let (_, end) = ranges.range(..=seq).next_back()?;
        (*end >= seq).then_some((*end).min(last))
    }
}
