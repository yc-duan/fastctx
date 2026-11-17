//! Anonymous occurrences pair in arrival order, without retaining a history of hashes.

use crate::timeline_window::Window;
use fastctx_types::session::SessionEvent;

#[derive(Clone)]
pub(crate) struct Anonymous {
    pub key: String,
    pub likeness: [u8; 32],
    pub event: SessionEvent,
    pub after: Option<String>,
    pub bytes: usize,
    pub published: bool,
}

impl Window {
    pub(crate) fn anonymous(&mut self, event: &SessionEvent, likeness: [u8; 32]) -> bool {
        let after = self.ordered_keys().last().cloned();
        self.next_anonymous = self.next_anonymous.wrapping_add(1);
        let key = format!("anonymous:{}", self.next_anonymous);
        self.anonymous.push(Anonymous {
            key: key.clone(),
            likeness,
            event: event.clone(),
            after,
            bytes: serde_json::to_vec(event).map_or(0, |bytes| bytes.len()),
            published: false,
        });
        self.reconcile_anonymous();
        self.bound_live();
        self.received_key = Some(key.clone());
        self.anonymous.iter().any(|event| event.key == key)
    }

    pub(crate) fn reconcile_anonymous(&mut self) {
        let entries = &mut self.entries;
        self.anonymous.retain(|live| {
            let matched = entries.iter_mut().find(|entry| {
                entry.likeness == Some(live.likeness) && !self.paired_anonymous.contains(&entry.place.item)
            });
            if let Some(entry) = matched {
                self.paired_anonymous.insert(entry.place.item.clone());
                if live.published {
                    entry.paired_key = Some(live.key.clone());
                }
                self.retired.push(live.key.clone());
                false
            } else {
                true
            }
        });
        self.paired_anonymous.retain(|item| entries.iter().any(|entry| &entry.place.item == item));
    }

    pub(crate) fn live_at_key(&self, key: &str) -> Option<&SessionEvent> {
        self.live
            .iter()
            .find(|event| crate::timeline_window::key(event) == key)
            .or_else(|| self.anonymous.iter().find(|event| event.key == key).map(|event| &event.event))
    }
}
