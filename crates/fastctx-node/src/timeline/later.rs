//! A conversation's record looked at again a while after it last grew
//! (design §28.3): a host may write its last lines after a turn ended, with
//! nothing to tell of them, and the timeline in the asset library is made
//! from what changes at its source, never from someone reading it. Each
//! conversation has at most one such watch, which asks for the timeline
//! about 5 s, 30 s and 2 min after the last growth, the steps starting over
//! when it grows again; each asking costs only a look at whether the record
//! changed.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use fastctx_types::ResidentId;
use tokio::time::Instant;

use crate::Node;

/// How long after the last growth each look comes.
const STEPS: [Duration; 3] = [Duration::from_secs(5), Duration::from_secs(30), Duration::from_secs(120)];

/// Each watched conversation, by when its record last grew.
static WATCHED: LazyLock<Mutex<HashMap<ResidentId, Instant>>> = LazyLock::new(Default::default);

/// `resident`'s record grew now: it is looked at again after each step.
pub(super) fn grew(node: &Arc<Node>, resident: &ResidentId) {
    let now = Instant::now();
    let watching = WATCHED.lock().expect("never poisoned").insert(resident.clone(), now).is_some();
    if watching {
        return;
    }
    let (node, resident) = (Arc::downgrade(node), resident.clone());
    fastctx_platform::task::spawn(async move {
        let mut from = now;
        let mut step = 0;
        loop {
            {
                let mut watched = WATCHED.lock().expect("never poisoned");
                let Some(last) = watched.get(&resident).copied() else { return };
                if last != from {
                    from = last;
                    step = 0;
                }
                if step == STEPS.len() {
                    watched.remove(&resident);
                    return;
                }
            }
            let due = from + STEPS[step];
            if Instant::now() < due {
                tokio::time::sleep_until(due).await;
                continue;
            }
            let Some(node) = node.upgrade() else {
                WATCHED.lock().expect("never poisoned").remove(&resident);
                return;
            };
            super::tail::wanted(&node, &resident);
            step += 1;
        }
    });
}
