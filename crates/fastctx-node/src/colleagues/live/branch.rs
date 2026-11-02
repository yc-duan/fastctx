//! A conversation of this machine as one branch of a collaboration: its
//! native agents, folded from its events as they come, how it stands, and
//! the goal it reports when it has one to tell.

use super::{SETTLE, Sender, stopping, tell};
use crate::Node;
use crate::colleagues::native;
use fastctx_net::watch_queue::{Events, TryRecvError};
use fastctx_types::{
    Address, ResidentId,
    access::Acting,
    collab::GoalSummary,
    collaboration::{Branch, Tree},
    ipc::Failure,
    residents::Origin,
    session::SessionEvent,
};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

/// How long reading the conversation's history and agents may take, as a
/// query of its collaboration allows.
const READING: Duration = Duration::from_secs(120);

/// How long after a read that could not follow the conversation's events it
/// is read again.
const AGAIN: Duration = Duration::from_secs(30);

pub(super) async fn keep(node: &Arc<Node>, id: &ResidentId, acting: &Acting, told: &Sender) -> Failure {
    let mut views = node.views.subscribe();
    let mut residents = node.residents.changes();
    let Some((about, _)) = node.residents.about(id) else {
        return crate::forget::gone(node, id);
    };
    if let Err(failure) = crate::colleagues::tree::visible(node, acting, id) {
        return failure;
    }
    let label = match &about.origin {
        Origin::Colleague { label, .. } => Some(label.clone()),
        Origin::User => None,
    };
    let mut native = Native::open(node, id).await;
    let mut goal = goal(node, id, label.is_some()).await;
    loop {
        if node.residents.about(id).is_none() {
            return crate::forget::gone(node, id);
        }
        if let Err(failure) = crate::colleagues::tree::visible(node, acting, id) {
            return failure;
        }
        let conversation = Branch {
            address: Address { machine: node.machine(), resident: id.clone() },
            label: label.clone(),
            born_at: Some(about.born_at),
            now: node.residents.now(id),
            goal: goal.clone(),
            plan: Some(crate::colleagues::plan::access(&about)),
            agents: native.fold.agents(),
            unavailable: native.unavailable.clone(),
        };
        tell(node, told, Tree { conversation, colleagues: Vec::new() });
        let mut next = tokio::select! {
            changed = views.changed() => if changed.is_err() { return stopping() } else { Next::Folded { goal: false } },
            changed = residents.changed() => if changed.is_err() { return stopping() } else { Next::Folded { goal: false } },
            next = native.next() => next,
        };
        if let Next::Folded { goal } = next {
            tokio::time::sleep(SETTLE).await;
            next = match native.drain() {
                Next::Folded { goal: drained } => Next::Folded { goal: goal || drained },
                Next::Lost => Next::Lost,
            };
        }
        views.borrow_and_update();
        residents.borrow_and_update();
        let goals = match next {
            Next::Folded { goal } => goal,
            Next::Lost => {
                native = Native::open(node, id).await;
                true
            }
        };
        if goals {
            goal = self::goal(node, id, label.is_some()).await;
        }
    }
}

/// The goal a colleague reports, where goals are offered.
async fn goal(node: &Arc<Node>, id: &ResidentId, colleague: bool) -> Result<Option<GoalSummary>, String> {
    if !colleague || !crate::goals::colleagues::offered() {
        return Ok(None);
    }
    match tokio::time::timeout(READING, crate::goals::colleagues::summary(node, id)).await {
        Ok(result) => result.map_err(|error| error.message),
        Err(_) => Err(fastctx_types::collab_prompt::GOAL_DEADLINE.into()),
    }
}

/// A conversation's native agents as read once and then folded from its
/// events, which it follows while it can.
struct Native {
    fold: native::Fold,
    unavailable: Vec<String>,
    events: Option<Events>,
    /// When a read that could not follow the events is read again.
    again: Instant,
}

enum Next {
    /// Events came and were folded in; whether one told of the goal.
    Folded { goal: bool },
    /// The events can no longer be followed: the conversation is read again.
    Lost,
}

impl Native {
    async fn open(node: &Arc<Node>, id: &ResidentId) -> Self {
        let read = native::open(node, id, Instant::now() + READING).await;
        Self { fold: read.fold, unavailable: read.unavailable, events: read.events, again: Instant::now() + AGAIN }
    }

    /// Waits for the next event and folds it in.
    async fn next(&mut self) -> Next {
        let Some(events) = &mut self.events else {
            tokio::time::sleep_until(self.again).await;
            return Next::Lost;
        };
        match events.recv().await {
            Ok(Some(event)) => Next::Folded { goal: fold_in(&mut self.fold, &event) },
            Ok(None) | Err(_) => {
                self.events = None;
                Next::Lost
            }
        }
    }

    /// Folds in the events that came meanwhile.
    fn drain(&mut self) -> Next {
        let Some(events) = &mut self.events else {
            return Next::Folded { goal: false };
        };
        let mut goal = false;
        loop {
            match events.try_recv() {
                Ok(event) => goal |= fold_in(&mut self.fold, &event),
                Err(TryRecvError::Empty) => return Next::Folded { goal },
                Err(TryRecvError::Lagged | TryRecvError::Disconnected) => {
                    self.events = None;
                    return Next::Lost;
                }
            }
        }
    }
}

/// Folds one event into the agents; whether it told of the goal.
fn fold_in(fold: &mut native::Fold, event: &SessionEvent) -> bool {
    fold.event(&event.happened, event.at, None, true);
    crate::goals::watch::is_goal(event)
}
