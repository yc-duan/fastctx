//! A conversation of this machine as one branch of a collaboration: its
//! native agents, folded from its events as they come, how it stands, and
//! the goal it reports when it has one to tell.

use super::{SETTLE, Sender, stopping, tell};
use crate::Node;
use crate::colleagues::native;
use fastctx_net::watch_queue::TryRecvError;
use fastctx_types::{
    Address, ResidentId,
    access::Acting,
    collab::GoalSummary,
    collaboration::{Branch, Tree},
    ipc::Failure,
    residents::Origin,
};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

/// How long reading the conversation's history and agents may take, as a
/// query of its collaboration allows.
const READING: Duration = Duration::from_secs(120);

/// How long after an incomplete read it is continued.
const AGAIN: Duration = Duration::from_secs(30);

pub(super) async fn keep(node: &Arc<Node>, id: &ResidentId, acting: &Acting, told: &Sender) -> Failure {
    let mut views = node.views.subscribe();
    let mut residents = node.residents.changes();
    let Some((about, _)) = node.residents.about(id) else {
        return crate::forget::gone(node, id);
    };
    let label = match &about.origin {
        Origin::Colleague { label, .. } => Some(label.clone()),
        Origin::User => None,
    };
    let mut native = Native { read: native::Read::default(), again: Instant::now() };
    let mut goal = Ok(None);
    let mut next = Next::Read;
    loop {
        if node.residents.about(id).is_none() {
            return crate::forget::gone(node, id);
        }
        if let Err(failure) = crate::colleagues::tree::visible(node, acting, id) {
            return failure;
        }
        // Read the goal before opening a history, and whenever a live event
        // says it changed. Child histories never delay the first goal.
        let goals = match next {
            Next::Folded { goal } => goal,
            Next::Read => native.read.events.is_none(),
        };
        if goals {
            goal = self::goal(node, id, label.is_some()).await;
            if let Err(failure) = crate::colleagues::tree::visible(node, acting, id) {
                return failure;
            }
        }
        let mut conversation = Branch {
            address: Address { machine: node.machine(), resident: id.clone() },
            label: label.clone(),
            born_at: Some(about.born_at),
            now: node.residents.now(id),
            goal: goal.clone(),
            plan: Some(crate::colleagues::plan::access(&about)),
            agents: native.read.fold.agents(),
            reading: native.read.reading(),
            unavailable: native.read.unavailable.clone(),
        };
        tell(node, told, Tree { conversation: conversation.clone(), colleagues: Vec::new() });
        if matches!(next, Next::Read) {
            native = Native::open(node, id, std::mem::take(&mut native.read), |fold, unavailable, reading| {
                if crate::colleagues::tree::visible(node, acting, id).is_ok() {
                    conversation.agents = fold.agents();
                    conversation.unavailable = unavailable.to_vec();
                    conversation.reading = reading;
                    tell(node, told, Tree { conversation: conversation.clone(), colleagues: Vec::new() });
                }
            })
            .await;
            next = Next::Folded { goal: std::mem::take(&mut native.read.goal_changed) };
            continue;
        }
        next = tokio::select! {
            changed = views.changed() => if changed.is_err() { return stopping() } else { Next::Folded { goal: false } },
            changed = residents.changed() => if changed.is_err() { return stopping() } else { Next::Folded { goal: false } },
            next = native.next() => next,
        };
        if let Next::Folded { goal } = next {
            tokio::time::sleep(SETTLE).await;
            next = match native.drain() {
                Next::Folded { goal: drained } => Next::Folded { goal: goal || drained },
                Next::Read => Next::Read,
            };
        }
        views.borrow_and_update();
        residents.borrow_and_update();
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

/// A conversation's native agents and live stream, retained between batches.
struct Native {
    read: native::Read,
    again: Instant,
}

enum Next {
    /// Events came and were folded in; whether one told of the goal.
    Folded { goal: bool },
    /// Continue unread child histories, rebuilding the parent only if its
    /// event stream was lost.
    Read,
}

impl Native {
    async fn open(
        node: &Arc<Node>,
        id: &ResidentId,
        read: native::Read,
        progress: impl FnMut(&native::Fold, &[String], bool),
    ) -> Self {
        let began = std::time::Instant::now();
        let read = native::open(node, id, Instant::now() + READING, read, progress).await;
        if began.elapsed() > Duration::from_secs(2) {
            node.log
                .line(format_args!("reading {id}'s agents for its collaboration took {}", crate::talk::lasted(began)));
        }
        Self { read, again: Instant::now() + AGAIN }
    }

    /// Waits for the next event and folds it in.
    async fn next(&mut self) -> Next {
        let reading = self.read.reading();
        let Some(events) = &mut self.read.events else {
            tokio::time::sleep_until(self.again).await;
            return Next::Read;
        };
        let event = tokio::select! {
            event = events.recv() => event,
            () = tokio::time::sleep_until(self.again), if reading => return Next::Read,
        };
        match event {
            Ok(Some(event)) => {
                self.read.event(&event);
                Next::Folded { goal: std::mem::take(&mut self.read.goal_changed) }
            }
            Ok(None) | Err(_) => {
                self.read.events = None;
                Next::Read
            }
        }
    }

    /// Folds in the events that came meanwhile.
    fn drain(&mut self) -> Next {
        while let Some(events) = &mut self.read.events {
            match events.try_recv() {
                Ok(event) => self.read.event(&event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Lagged | TryRecvError::Disconnected) => {
                    self.read.events = None;
                    return Next::Read;
                }
            }
        }
        Next::Folded { goal: std::mem::take(&mut self.read.goal_changed) }
    }
}
