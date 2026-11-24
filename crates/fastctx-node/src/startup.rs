//! Recovery belongs to the conversation whose records it settles. The listener
//! and unrelated conversations never wait for another host's replay or lookup.

use crate::{Node, residents::Residents};
use fastctx_types::ResidentId;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

pub(crate) struct Recovery {
    pending: Mutex<HashMap<ResidentId, Arc<Notify>>>,
    initial: Mutex<Option<Vec<Conversation>>>,
    changed: Notify,
}

struct Conversation {
    record: crate::residents::OnRecord,
    turns: Vec<String>,
    requested: Arc<Notify>,
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
                Conversation { record, turns, requested: Arc::new(Notify::new()) }
            })
            .collect();
        Self {
            pending: Mutex::new(
                initial.iter().map(|entry| (entry.record.id.clone(), entry.requested.clone())).collect(),
            ),
            initial: Mutex::new(Some(initial)),
            changed: Notify::new(),
        }
    }

    pub(crate) fn is_ready(&self, id: &ResidentId) -> bool {
        !self.pending.lock().expect("startup recovery").contains_key(id)
    }

    /// Let a conversation someone is opening or speaking to use the
    /// recovery slot reserved for interactive requests.
    pub(crate) fn request_input(&self, id: &ResidentId) {
        if let Some(requested) = self.pending.lock().expect("startup recovery").get(id) {
            requested.notify_one();
        }
    }

    pub(crate) async fn wait_for_input(&self, id: &ResidentId) {
        self.request_input(id);
        self.wait(id).await;
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

async fn recovery_slot(
    slots: Arc<Semaphore>,
    requested_slot: Arc<Semaphore>,
    requested: &Notify,
) -> OwnedSemaphorePermit {
    tokio::select! {
        slot = slots.acquire_owned() => slot,
        slot = async {
            requested.notified().await;
            requested_slot.acquire_owned().await
        } => slot,
    }
    .expect("never closed")
}

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
    let slots = Arc::new(Semaphore::new(AT_ONCE));
    let requested_slot = Arc::new(Semaphore::new(1));
    let initial = node.startup.initial.lock().expect("startup recovery").take().unwrap_or_default();
    for Conversation { record: mut resident, turns, requested } in initial {
        let node = node.clone();
        let slots = slots.clone();
        let requested_slot = requested_slot.clone();
        workers.spawn(async move {
            let id = resident.id.clone();
            let finished = Finished { node: &node, id: id.clone() };
            let mut again = crate::again::Again::default();
            let mut told = None;
            // An import that cannot finish yet holds no recovery place meanwhile.
            while let Err(error) = crate::conversation_store::transfer::wait(&node, &id).await {
                if told.as_ref() != Some(&error.message) {
                    node.log.line(format_args!("waiting for conversation import {id}: {}", error.message));
                    told = Some(error.message);
                }
                tokio::time::sleep(again.next()).await;
            }
            let _slot = recovery_slot(slots, requested_slot, &requested).await;
            let began = std::time::Instant::now();
            if let Some(reason) = crate::conversation_store::transfer::rejection(node.world.path(), id.as_str()) {
                node.log
                    .line(format_args!("conversation import {id} was rejected: {reason}; startup recovery skipped"));
                return;
            }
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
            crate::control_queue::recover(&node, &id).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{Future, poll_fn};
    use std::pin::pin;
    use std::task::Poll;
    use std::time::Duration;

    #[tokio::test]
    async fn input_can_start_its_recovery_while_background_slots_are_full() {
        let id: ResidentId = "claude-f1234567".parse().unwrap();
        let wanted = Arc::new(Notify::new());
        let recovery = Recovery {
            pending: Mutex::new(HashMap::from([(id.clone(), wanted.clone())])),
            initial: Mutex::new(None),
            changed: Notify::new(),
        };
        let background = Arc::new(Semaphore::new(0));
        let priority = Arc::new(Semaphore::new(1));
        let mut slot = pin!(recovery_slot(background, priority.clone(), &wanted));
        assert!(poll_fn(|cx| Poll::Ready(slot.as_mut().poll(cx).is_pending())).await);
        assert_eq!(priority.available_permits(), 1);

        let mut input = pin!(recovery.wait_for_input(&id));
        assert!(poll_fn(|cx| Poll::Ready(input.as_mut().poll(cx).is_pending())).await);
        let permit = tokio::time::timeout(Duration::from_secs(1), slot).await.unwrap();
        assert_eq!(priority.available_permits(), 0);
        assert!(!recovery.is_ready(&id));

        recovery.finished(&id);
        input.await;
        drop(permit);
        assert_eq!(priority.available_permits(), 1);
    }
}
