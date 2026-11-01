//! Committed terminals retain work independently of their initiating future.
//! One retry worker per resident repairs small pending keys in turn order.
use crate::Node;
use fastctx_platform::doing::{MemoryMutex, Rank};
use fastctx_types::ResidentId;
use std::{collections::HashMap, io, sync::Arc, time::Duration};

pub(crate) struct Work {
    running: MemoryMutex<HashMap<ResidentId, bool>>,
}
impl Default for Work {
    fn default() -> Self {
        Self { running: MemoryMutex::new(Rank::RECORDS, "terminal workers", "node", HashMap::new()) }
    }
}

/// The callback retains the caller's speech permit through the actual commit.
/// Retirement cannot pass the writer after the initiating future is cancelled.
pub(crate) fn notification(node: &Arc<Node>, id: &ResidentId) -> io::Result<impl FnOnce() + Send + 'static> {
    let held = node.residents.keep_speaking(id)?;
    let (runtime, node, id) = (tokio::runtime::Handle::current(), node.clone(), id.clone());
    Ok(move || {
        schedule(&runtime, &node, &id);
        drop(held);
    })
}

fn schedule(runtime: &tokio::runtime::Handle, node: &Arc<Node>, id: &ResidentId) {
    {
        let mut running = node.delivery_work.running.lock();
        if let Some(dirty) = running.get_mut(id) {
            *dirty = true;
            return;
        }
        running.insert(id.clone(), false);
    }
    let (node, id) = (node.clone(), id.clone());
    fastctx_platform::task::spawn_on(runtime, async move {
        node.ready_for(&id).await;
        let mut problem = None;
        let mut failed_turns = HashMap::<String, String>::new();
        loop {
            let Some(speaking) = node.residents.speaking(&id) else {
                node.delivery_work.running.lock().remove(&id);
                return;
            };
            let mut failed = false;
            let result = async {
                let directory = node.world.conversation(&id);
                let mut after: Option<String> = None;
                loop {
                    let (reading, cursor) = (directory.clone(), after.clone());
                    let turns = fastctx_platform::task::spawn_blocking(move || {
                        super::terminals::pending_turns(&reading, cursor.as_deref())
                    })
                    .await
                    .map_err(io::Error::other)??;
                    let Some(last) = turns.last() else { break };
                    after = Some(last.clone());
                    for turn in turns {
                        let _speaking = speaking.lock().await;
                        match crate::turns::settle_terminal_locked(&node, &id, &turn, false).await {
                            Ok(()) => {
                                failed_turns.remove(&turn);
                            }
                            Err(error) => {
                                failed = true;
                                let error = error.to_string();
                                if failed_turns.get(&turn) != Some(&error) {
                                    node.log.line(format_args!("settling {id}'s turn {turn}: {error}"));
                                    // Only diagnostics are evicted; the durable work remains pending.
                                    if failed_turns.len() == 64 {
                                        failed_turns.clear();
                                    }
                                    failed_turns.insert(turn, error);
                                }
                            }
                        }
                    }
                }
                Ok::<(), io::Error>(())
            }
            .await;
            if let Err(error) = result {
                let error = error.to_string();
                if problem.as_ref() != Some(&error) {
                    node.log.line(format_args!("settling {id}'s terminal records: {error}"));
                    problem = Some(error);
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            if failed {
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            let mut running = node.delivery_work.running.lock();
            if running.get_mut(&id).is_some_and(std::mem::take) {
                continue;
            }
            running.remove(&id);
            return;
        }
    });
}

pub(crate) fn recover(node: &Arc<Node>) {
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let mut after = (String::new(), String::new());
        let mut last = String::new();
        loop {
            let (database, cursor) = (node.database.clone(), after.clone());
            let page =
                match fastctx_platform::task::spawn_blocking(move || super::terminals::pending(&database, &cursor))
                    .await
                {
                    Ok(Ok(page)) => page,
                    result => {
                        node.log.line(format_args!("listing pending terminal records: {result:?}"));
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        continue;
                    }
                };
            let Some(next) = page.last() else { return };
            after = next.clone();
            for (scope, _) in page {
                if scope != last
                    && let Ok(id) = scope.parse()
                {
                    schedule(&tokio::runtime::Handle::current(), &node, &id);
                }
                last = scope;
            }
        }
    });
}
