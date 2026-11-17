//! Deadline lookup shared by committed snapshots and the expiration worker.

use std::{collections::BTreeMap, sync::Arc};

use fastctx_net::directory::{Assist, Directory};
use jiff::Timestamp;

#[derive(Default)]
pub(crate) struct AssistEnds(BTreeMap<Timestamp, Vec<(usize, Arc<Assist>)>>);

impl AssistEnds {
    pub(crate) fn new(directory: &Directory) -> Self {
        let mut deadlines: BTreeMap<Timestamp, Vec<(usize, Arc<Assist>)>> = BTreeMap::new();
        for (at, assist) in directory.assists.iter().enumerate() {
            if let Some(until) = assist.until {
                deadlines.entry(until).or_default().push((at, assist.clone()));
            }
        }
        Self(deadlines)
    }

    pub(crate) fn next(&self) -> Option<Timestamp> {
        self.0.first_key_value().map(|(at, _)| *at)
    }

    pub(crate) fn due(&self, now: Timestamp) -> Vec<Arc<Assist>> {
        let mut due: Vec<_> = self.0.range(..=now).flat_map(|(_, assists)| assists.iter()).collect();
        due.sort_unstable_by_key(|(at, _)| *at);
        due.into_iter().map(|(_, assist)| assist.clone()).collect()
    }
}
