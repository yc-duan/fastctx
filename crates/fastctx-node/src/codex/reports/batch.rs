//! Bounded prefixes for host-report commits. Encoding happens off the task
//! threads and outside the queue lock; one large report travels alone.

use crate::codex::HostEvent;
use std::io;

pub(super) struct Batch {
    pub items: Vec<(String, HostEvent, bool)>,
    pub reports: Vec<(String, Vec<u8>)>,
}

impl Batch {
    pub fn take(mut next: impl FnMut(usize) -> Option<(String, HostEvent)>, fence: &str) -> io::Result<Self> {
        let mut batch = Self { items: Vec::new(), reports: Vec::new() };
        let mut bytes = 0;
        for index in 0..16 {
            let Some((id, event)) = next(index) else { break };
            let body = serde_json::to_vec(&event)?;
            let size = id.len() + body.len() + 128;
            if !batch.items.is_empty() && bytes + size > 64 * 1024 {
                break;
            }
            bytes += size;
            let durable = crate::codex::durable(&event);
            if durable {
                batch.reports.push((id.clone(), body));
            }
            let last = id == fence;
            batch.items.push((id, event, durable));
            if last {
                break;
            }
        }
        Ok(batch)
    }
}
