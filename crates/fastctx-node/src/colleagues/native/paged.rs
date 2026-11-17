//! The parent's history as the facts a native-agent tree is folded from:
//! when each call began, and what was said of its agents. Read a page at a
//! time off the conversation's timeline where it is read here (design
//! §28.52 52-1), each event let go as its fact is taken, so a long history
//! is never whole in memory; otherwise read whole from its sources and
//! taken down to the same facts at once.

use super::Fold;
use crate::Node;
use crate::timeline::viewed::{self, Next, Reader};
use crate::watch::Events;
use fastctx_types::ipc::Failure;
use fastctx_types::ResidentId;
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use jiff::Timestamp;
use std::sync::Arc;

/// What of one event a tree is folded from.
pub(super) enum Fact {
    /// A call of the conversation's own loop began.
    Call(String, Timestamp),
    /// Its agents were said to start, act, end or stand somehow.
    Agents(SessionEvent),
}

/// What of `event` a tree is folded from, if anything.
fn fact(event: SessionEvent) -> Option<Fact> {
    match &event.happened {
        Happened::Began { item, .. } | Happened::Finished { item, .. } | Happened::Updated { item, .. } => {
            item.started_at.map(|started| Fact::Call(item.id.clone(), started))
        }
        Happened::Changed {
            change:
                SessionChange::AgentStarted { .. }
                | SessionChange::AgentEvent { .. }
                | SessionChange::AgentEnded { .. }
                | SessionChange::Agents { .. },
            ..
        } => Some(Fact::Agents(event)),
        _ => None,
    }
}

impl Fold {
    /// Folds in `facts`, the parent's history taken down to them.
    pub(super) fn facts(&mut self, facts: &[Fact]) {
        for fact in facts {
            match fact {
                Fact::Call(id, started) => self.call((None, id.clone()), *started),
                Fact::Agents(event) => self.event(&event.happened, event.at, None, false),
            }
        }
    }
}

/// The parent's history read for its tree.
pub(super) struct Parent {
    pub(super) facts: Vec<Fact>,
    /// How many events were read.
    pub(super) read: usize,
    pub(super) events: Events,
    pub(super) unread: bool,
    pub(super) unsettled: bool,
    pub(super) unavailable: Vec<String>,
}

/// The history of `resident`, the parent, read for its tree: off its
/// timeline where it is read here, otherwise whole from its sources. The
/// watch begins before either is read, so nothing falls between.
pub(super) async fn parent(node: &Arc<Node>, resident: &ResidentId) -> Result<Parent, Failure> {
    if let Some(parent) = paged(node, resident).await? {
        return Ok(parent);
    }
    let history = crate::talk::history_quietly(node, resident).await?;
    let read = history.history.len();
    let facts = history.history.into_iter().filter_map(fact).collect();
    Ok(Parent {
        facts,
        read,
        events: history.events,
        unread: history.unread,
        unsettled: history.unsettled,
        unavailable: history.unavailable,
    })
}

/// How many times a reading begins again as the timeline is written under
/// it, before it holds the timeline still.
const TRIES: u32 = 3;

/// The history of `resident` off its timeline, a page at a time: none where
/// the timeline is not read here.
async fn paged(node: &Arc<Node>, resident: &ResidentId) -> Result<Option<Parent>, Failure> {
    if !node.residents.about(resident).is_some_and(|(about, _)| about.world_managed()) {
        return Ok(None);
    }
    let (events, _waiting) = crate::talk::requested(node, resident, node.watchers.watch(resident))?;
    let Some((held, unread)) = viewed::brought(node, resident).await? else { return Ok(None) };
    let (mut reader, mut facts, mut read) = (Reader::forward(held), Vec::new(), 0usize);
    let (mut tries, mut _steady) = (0, None);
    loop {
        let (back, next, taken, counted) = page(reader).await?;
        reader = back;
        match next {
            Next::Page(_) => {
                facts.extend(taken);
                read += counted;
            }
            Next::End => {
                return Ok(Some(Parent { facts, read, events, unread, unsettled: false, unavailable: Vec::new() }));
            }
            Next::Changed => {
                tries += 1;
                if tries >= TRIES {
                    return Err(viewed::kept_changing(resident));
                }
                if tries + 1 == TRIES {
                    _steady = Some(viewed::steady(resident).await);
                }
                facts.clear();
                read = 0;
                reader.restart();
            }
            Next::Missing => return Ok(None),
        }
    }
}

/// The next page of `reader`, taken down to its facts as it is read, on a
/// thread that may block.
async fn page(mut reader: Reader) -> Result<(Reader, Next<SessionEvent>, Vec<Fact>, usize), Failure> {
    fastctx_platform::task::spawn_blocking(move || {
        let (mut taken, mut counted) = (Vec::new(), 0usize);
        let next = reader.next_as(&mut |event| {
            counted += 1;
            taken.extend(fact(event));
            None
        })?;
        Ok((reader, next, taken, counted))
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
}
