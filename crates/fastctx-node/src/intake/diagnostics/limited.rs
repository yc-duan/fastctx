//! The public diagnostic census waits briefly, then returns the part it obtained.

use super::*;
use std::ops::Bound::{Excluded, Unbounded};

const ROWS: usize = 2048;
const BYTES: usize = 1024 * 1024;

pub(crate) struct Snapshot {
    pub calls: Vec<CallWork>,
    pub partial: bool,
    pub next: Option<u64>,
}

pub(crate) fn snapshot_bounded(after: Option<u64>) -> Snapshot {
    let deadline = std::time::Instant::now() + Duration::from_millis(200);
    let waiting = fastctx_platform::doing::task_waits_bounded(deadline);
    let mut partial = waiting.partial;
    let after = after.unwrap_or(0);
    let mut calls = Vec::new();
    if let Some(state) = fastctx_platform::doing::snapshot_lock(&STATE, deadline) {
        let now = tokio::time::Instant::now();
        for (id, entry) in state.calls.range((Excluded(after), Unbounded)).take(ROWS + 1) {
            calls.push(row(*id, entry, &waiting.tasks, now));
        }
    } else {
        partial = true;
    }
    let (subscriptions, incomplete) = crate::local_subscriptions::snapshot_page(after, ROWS + 1, deadline);
    calls.extend(subscriptions);
    partial |= incomplete;
    calls.sort_by_key(|call| call.id);
    let mut kept = Vec::new();
    let mut bytes = 0;
    let mut more = false;
    for mut call in calls {
        // Diagnostic labels are normally short. Older retained labels may be
        // larger; expose their prefix instead of preventing the whole census.
        for field in [&mut call.kind, &mut call.waiting, &mut call.account, &mut call.machine]
            .into_iter()
            .chain(call.conversation.iter_mut())
        {
            if field.len() > 4096 {
                let mut end = 4093;
                while !field.is_char_boundary(end) {
                    end -= 1;
                }
                field.truncate(end);
                field.push('…');
                partial = true;
            }
        }
        let size = call.kind.len()
            + call.waiting.len()
            + call.account.len()
            + call.machine.len()
            + call.conversation.as_ref().map_or(0, String::len)
            + 256;
        if kept.len() == ROWS || bytes + size > BYTES {
            more = true;
            break;
        }
        bytes += size;
        kept.push(call);
    }
    let next = more.then(|| kept.last().map_or(after, |call| call.id));
    Snapshot { calls: kept, partial: partial || more, next }
}
