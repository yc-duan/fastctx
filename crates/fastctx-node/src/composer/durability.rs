//! Coalesced disk barriers for accepted text updates. In normal operation, power
//! loss can discard at most the last second of typing. The barrier starts after
//! 500 ms; a blocked or failed barrier is reported and remains pending.

use fastctx_storage::{Database, Error, Priority};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const AFTER: Duration = Duration::from_millis(500);
const WITHIN: Duration = Duration::from_secs(1);
const RETRY: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Pending {
    first: Option<Instant>,
    running: bool,
}

#[derive(Default)]
pub(super) struct Durability {
    pending: Mutex<Pending>,
}

pub(super) fn committed(node: &Arc<crate::Node>, runtime: &tokio::runtime::Handle) {
    let log = node.log.clone();
    node.composers.durability.request(&node.database, runtime, move |error| {
        log.line(format_args!("synchronizing accepted composer text: {error}"));
    });
}

impl Durability {
    /// The first unsettled edit sets the deadline; later typing cannot postpone
    /// it. Idle composers have no timer or writes. A failed barrier remains pending.
    pub(super) fn request(
        self: &Arc<Self>,
        database: &Database,
        runtime: &tokio::runtime::Handle,
        failed: impl Fn(&Error) + Send + 'static,
    ) {
        {
            let mut pending = self.pending.lock().expect("composer disk barriers");
            pending.first.get_or_insert_with(Instant::now);
            if pending.running {
                return;
            }
            pending.running = true;
        }
        let (state, database) = (self.clone(), database.clone());
        fastctx_platform::task::spawn_on(runtime, async move {
            loop {
                let due = {
                    let mut pending = state.pending.lock().expect("composer disk barriers");
                    let Some(first) = pending.first else {
                        pending.running = false;
                        return;
                    };
                    first + AFTER
                };
                tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
                let first = state.pending.lock().expect("composer disk barriers").first.take().expect("pending edit");
                let barrier = database.clone();
                let mut writing =
                    fastctx_platform::task::spawn_blocking(move || barrier.settle_now(Priority::Interactive));
                let result =
                    match tokio::time::timeout_at(tokio::time::Instant::from_std(first + WITHIN), &mut writing).await {
                        Ok(result) => result,
                        Err(_) => {
                            failed(&Error::new("accepted composer text exceeded its one-second disk-sync deadline"));
                            writing.await
                        }
                    };
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(error) => Some(Error::new(error.to_string())),
                };
                if let Some(error) = error {
                    {
                        let mut pending = state.pending.lock().expect("composer disk barriers");
                        pending.first = Some(pending.first.map_or(first, |next| next.min(first)));
                    }
                    failed(&error);
                    tokio::time::sleep(RETRY).await;
                }
                // Commits while the barrier was in flight remain pending for the
                // next pass; clearing that state would race a later NORMAL commit.
            }
        });
    }
}
