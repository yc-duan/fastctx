//! Recovery belongs to the conversation whose records it settles. The listener
//! and unrelated conversations never wait for another host's replay or lookup.

use crate::{Node, residents::Residents};
use fastctx_types::ResidentId;
use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub(crate) struct Recovery {
    pending: Mutex<HashSet<ResidentId>>,
    initial: Mutex<Option<Vec<Conversation>>>,
    changed: Notify,
}

struct Conversation {
    record: crate::residents::OnRecord,
    turns: Vec<String>,
}

impl Recovery {
    pub(crate) fn new(residents: &Residents) -> Self {
        let mut unfinished = std::collections::HashMap::<_, Vec<_>>::new();
        for (id, turn) in residents.unfinished() {
            unfinished.entry(id).or_default().push(turn);
        }
        let initial: Vec<_> = residents
            .everyone()
            .map(|record| {
                let turns = unfinished.remove(&record.id).unwrap_or_default();
                Conversation { record, turns }
            })
            .collect();
        Self {
            pending: Mutex::new(initial.iter().map(|entry| entry.record.id.clone()).collect()),
            initial: Mutex::new(Some(initial)),
            changed: Notify::new(),
        }
    }

    pub(crate) fn is_ready(&self, id: &ResidentId) -> bool {
        !self.pending.lock().expect("startup recovery").contains(id)
    }

    pub(crate) async fn wait(&self, id: &ResidentId) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_ready(id) {
                return;
            }
            changed.await;
        }
    }

    fn finished(&self, id: &ResidentId) {
        self.pending.lock().expect("startup recovery").remove(id);
        self.changed.notify_waiters();
    }
}

/// How many conversations recover at once: each may ask its host to read a
/// thread back, and a machine holding hundreds must not ask for them all
/// together.
const AT_ONCE: usize = 8;

/// Lets a conversation's calls through once its recovery ends, however it
/// ends: one that failed part way is not held shut for good.
struct Finished<'a> {
    node: &'a Node,
    id: ResidentId,
}

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.node.startup.finished(&self.id);
    }
}

pub(crate) async fn recover(node: &Arc<Node>) {
    let mut workers = tokio::task::JoinSet::new();
    let slots = Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
    let initial = node.startup.initial.lock().expect("startup recovery").take().unwrap_or_default();
    for Conversation { record: mut resident, turns } in initial {
        let node = node.clone();
        let slots = slots.clone();
        workers.spawn(async move {
            let _slot = slots.acquire_owned().await.expect("never closed");
            let id = resident.id.clone();
            let finished = Finished { node: &node, id: id.clone() };
            let began = std::time::Instant::now();
            crate::codex::adopt(&node, std::slice::from_ref(&resident)).await;
            if let Some(session) = &resident.about.session {
                match resident.about.host.agent.as_str() {
                    "codex" => crate::codex::replay_thread(&node, session).await,
                    "claude" => crate::claude::replay_session(&node, session).await,
                    _ => {}
                }
            }
            if let Some((about, town)) = node.residents.about(&id) {
                resident.about = about;
                resident.town = town;
            }
            crate::speech::restore(&node, std::slice::from_ref(&resident)).await;
            let mut turns: BTreeSet<_> = turns.into_iter().map(|turn| (id.clone(), turn)).collect();
            if let Some(turn) = node.residents.now(&id).and_then(|now| now.turn) {
                turns.insert((id.clone(), turn));
            }
            crate::turns::recover(&node, turns.into_iter().collect()).await;
            crate::claude::recover_startups(&node, &resident);
            crate::control_queue::recover(&node, &id);
            crate::speech::recover(&node, vec![resident]).await;
            drop(finished);
            crate::control_queue::poke(&node, &id);
            if began.elapsed().as_secs() >= 1 {
                node.log.line(format_args!("startup recovery for {id} finished in {:?}", began.elapsed()));
            }
        });
    }
    while let Some(result) = workers.join_next().await {
        if let Err(error) = result {
            node.log.line(format_args!("conversation startup recovery did not finish: {error}"));
        }
    }
}
