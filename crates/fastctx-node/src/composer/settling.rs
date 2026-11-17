//! Words a device leaves in a busy conversation's queue, from an input box
//! that keeps them until it is told how they went (the native document
//! input), are taken without waiting for the disk (受理, design §3,
//! §28.46): committed, the device sees them queued at once. Its answer —
//! the moment it lets them go — waits for a disk barrier that has made
//! them durable (已落盘). One barrier serves every commit done before it
//! began. Until then no state a device is shown names the send's
//! operation: a device lets go of a send whose operation a state shows
//! settled, so none is shown before the disk has it.

use fastctx_storage::Priority;
use fastctx_types::ResidentId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long an answer waits for its barrier before the device is told to
/// ask again: it keeps its words meanwhile, and asks under the same
/// operation.
const WITHIN: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_millis(100);
const RETRY_AT_MOST: Duration = Duration::from_secs(5);
/// Not yet committed: past every barrier's count.
const UNCOMMITTED: u64 = u64::MAX;

pub(super) struct Settling {
    state: Mutex<State>,
    /// The commits made durable so far, by their count.
    settled: tokio::sync::watch::Sender<u64>,
    /// One barrier after this process began, before any state is shown:
    /// what the last process committed without the disk may not be on it.
    began: tokio::sync::OnceCell<()>,
}

#[derive(Default)]
struct State {
    /// The commits counted so far.
    committed: u64,
    running: bool,
    /// The operations of sends committed without the disk and not yet past
    /// a barrier, by conversation, each with the count of its commit.
    withheld: HashMap<ResidentId, HashMap<String, u64>>,
}

impl Default for Settling {
    fn default() -> Self {
        Self { state: Mutex::default(), settled: tokio::sync::watch::channel(0).0, began: tokio::sync::OnceCell::new() }
    }
}

/// Keeps the send `operation` of `id` out of every state shown until a
/// barrier has its commit: called before it is committed. Whether this is
/// the first that withholds it: one withheld already — an earlier attempt
/// whose outcome is not known — stays withheld whatever becomes of this one.
pub(super) fn withhold(node: &crate::Node, id: &ResidentId, operation: &str) -> bool {
    let mut state = node.composers.settling.state.lock().expect("composer settling");
    let operations = state.withheld.entry(id.clone()).or_default();
    if operations.contains_key(operation) {
        return false;
    }
    operations.insert(operation.to_string(), UNCOMMITTED);
    true
}

/// The conversation is gone: nothing of it is withheld.
pub(super) fn forget(node: &crate::Node, id: &ResidentId) {
    node.composers.settling.state.lock().expect("composer settling").withheld.remove(id);
}

/// Its commit failed for certain, and it was the first to withhold it: the
/// send is not there to withhold.
pub(super) fn release(node: &crate::Node, id: &ResidentId, operation: &str) {
    let mut state = node.composers.settling.state.lock().expect("composer settling");
    if let Some(operations) = state.withheld.get_mut(id) {
        operations.remove(operation);
        if operations.is_empty() {
            state.withheld.remove(id);
        }
    }
}

/// Takes out of `operations` those of `id` no barrier has yet.
pub(super) fn shown(node: &crate::Node, id: &ResidentId, operations: &mut Vec<fastctx_types::composer::sync::Operation>) {
    let state = node.composers.settling.state.lock().expect("composer settling");
    if let Some(withheld) = state.withheld.get(id) {
        operations.retain(|operation| !withheld.contains_key(&operation.operation));
    }
}

/// Waits for the barrier after this process began (`Settling::began`).
pub(super) async fn begun(node: &Arc<crate::Node>) -> Result<(), fastctx_types::ipc::Failure> {
    let settling = node.composers.settling.clone();
    let database = node.database.clone();
    settling
        .began
        .get_or_try_init(|| async move {
            fastctx_platform::task::spawn_blocking(move || database.settle_now(Priority::Interactive))
                .await
                .map_err(|error| fastctx_types::ipc::Failure::unanswered(error.to_string()))?
                .map_err(|error| fastctx_types::ipc::Failure::unanswered(error.to_string()))
        })
        .await
        .map(|_| ())
}

/// Returns once whatever this node's database committed before the call is
/// on the disk, the send `withheld` among it shown from then on; an error
/// the device takes as "ask again" when the disk does not confirm it in
/// time.
pub(super) async fn durable(node: &Arc<crate::Node>, withheld: Option<(&ResidentId, &str)>) -> Result<(), fastctx_types::ipc::Failure> {
    let settling = node.composers.settling.clone();
    let mine = {
        let mut state = settling.state.lock().expect("composer settling");
        state.committed += 1;
        let mine = state.committed;
        if let Some((id, operation)) = withheld
            && let Some(count) = state.withheld.get_mut(id).and_then(|operations| operations.get_mut(operation))
        {
            *count = (*count).min(mine);
        }
        if !state.running {
            state.running = true;
            let (settling, node) = (settling.clone(), Arc::downgrade(node));
            fastctx_platform::task::spawn(async move { settle(settling, node).await });
        }
        mine
    };
    let mut settled = settling.settled.subscribe();
    match tokio::time::timeout(WITHIN, settled.wait_for(|settled| *settled >= mine)).await {
        Ok(Ok(_)) => Ok(()),
        _ => Err(fastctx_types::ipc::Failure::unanswered(
            "the disk has not yet confirmed the words; they are kept and asked for again",
        )),
    }
}

async fn settle(settling: Arc<Settling>, node: std::sync::Weak<crate::Node>) {
    let mut retry = RETRY;
    loop {
        let through = {
            let mut state = settling.state.lock().expect("composer settling");
            if *settling.settled.borrow() >= state.committed {
                state.running = false;
                return;
            }
            state.committed
        };
        let Some(database) = node.upgrade().map(|node| node.database.clone()) else {
            settling.state.lock().expect("composer settling").running = false;
            return;
        };
        let result = fastctx_platform::task::spawn_blocking(move || database.settle_now(Priority::Interactive)).await;
        match result {
            Ok(Ok(())) => {
                retry = RETRY;
                {
                    // Past the barrier: shown from now on.
                    let mut state = settling.state.lock().expect("composer settling");
                    state.withheld.retain(|_, operations| {
                        operations.retain(|_, count| *count > through);
                        !operations.is_empty()
                    });
                }
                settling.settled.send_modify(|settled| *settled = (*settled).max(through));
            }
            failed => {
                let error = match failed {
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => error.to_string(),
                    Ok(Ok(())) => unreachable!(),
                };
                if let Some(node) = node.upgrade() {
                    node.log.line(format_args!("making queued words durable: {error}; tried again"));
                }
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(RETRY_AT_MOST);
            }
        }
    }
}
