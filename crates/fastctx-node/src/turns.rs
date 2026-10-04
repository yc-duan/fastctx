//! How a turn of a session the World holds ends, the one way for all three
//! ways it can (`world-colleagues.md`, 一轮怎样结束): its host reports the
//! end; the host's process stops under it; or the node stops while it runs
//! and finds it unfinished when it starts again. Each ends here: the
//! resident's records say how, everyone watching sees it, and everyone owed
//! the turn's end gets the agent's last message, verbatim.
//!
//! The answers are kept in the resident's outbox before the turn's debt is
//! cleared, and each leaves it once its reader's machine has it
//! (`crate::inbox`); an answer's id comes from the turn and the reader, so
//! however often a turn's end is told, each reader hears it once.
//!
//! What the host reports about the session itself, rather than its work,
//! goes into the resident's `now` and to everyone watching; what must be
//! heard at once also goes to everyone owed the running turn's end.

use std::sync::Arc;

use fastctx_types::layout::ResidentDir;
use fastctx_types::residents::{Delivery, MessageKind, MessageLine, Notice, Origin, Owing, Pending, Trouble};
use fastctx_types::session::{Happened, Health, SessionEvent, TurnEnd, TurnHow};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use crate::Node;
use crate::speech::{self, Owed};

/// Something a host reported about a resident's session.
pub async fn happened(node: &Arc<Node>, id: &ResidentId, event: SessionEvent) {
    match event.happened {
        Happened::TurnEnded { turn, end } => ended(node, id, &turn, end).await,
        Happened::Health { turn, health: report } => {
            // A report can come before the answer naming the turn words went
            // into: whoever they are from is recorded first.
            let Some(speaking) = node.residents.speaking(id) else { return };
            let _speaking = speaking.lock().await;
            health(node, id, turn, report);
        }
        Happened::TurnStarted { ref turn } => {
            if let Err(error) = node.residents.turn_started(&node.world, id, turn) {
                node.log.line(format_args!("recording that {id} started a turn: {error}"));
            }
            node.watchers.publish(id, &event);
        }
        _ => node.watchers.publish(id, &event),
    }
}

/// A turn of a resident's session ended. An empty `turn` is one whose id
/// was never known.
pub async fn ended(node: &Arc<Node>, id: &ResidentId, turn: &str, end: TurnEnd) {
    let at = Timestamp::now();
    if !turn.is_empty() {
        let event = SessionEvent { at, happened: Happened::TurnEnded { turn: turn.to_string(), end: end.clone() } };
        node.watchers.publish(id, &event);
    }
    crate::desk::turn_ended(node, id, turn);
    let Some((about, town)) = node.residents.about(id) else {
        return;
    };
    let here = node.machine();
    let requester_here = match &about.origin {
        Origin::Colleague { asked_by, .. } => node.residents.requester_here(asked_by, &here),
        Origin::User => true,
    };
    let trouble = match (&end.how, &end.reason) {
        (TurnHow::Completed, _) => None,
        (how, Some(reason)) => Some(Trouble { at, text: format!("turn {how}: {reason}") }),
        (how, None) => Some(Trouble { at, text: format!("turn {how}") }),
    };
    let directory = node.world.resident(town.as_ref(), id);
    let readers = {
        let Some(speaking) = node.residents.speaking(id) else { return };
        let _speaking = speaking.lock().await;
        let readers = node.residents.readers(id, turn);
        // Kept before the debt is cleared: a node that stops in between
        // finds both, and telling the same end again is the same answer.
        if let Err(error) = keep_answers(node, id, &about.origin, &directory, turn, turn, &end, &readers).await {
            node.log.line(format_args!("keeping the end of {id}'s turn {turn} for its readers: {error}"));
            return;
        }
        match node.residents.take_owed(&node.world, id, turn, requester_here, trouble) {
            Ok(readers) => readers,
            Err(error) => {
                node.log.line(format_args!("recording how {id}'s turn ended: {error}"));
                return;
            }
        }
    };
    if matches!(about.origin, Origin::Colleague { .. }) {
        let line =
            MessageLine { at, what: MessageKind::Ended { turn: turn.to_string(), end: end.clone(), owed: readers } };
        if let Err(error) = crate::residents::append_message(&directory, &line) {
            node.log.line(format_args!("recording how {id}'s turn ended: {error}"));
        }
    }
    // Telling may wait for a reader's own session; the turn's end is
    // settled already.
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move { pay(&node, &id, &directory).await });
}

/// Keeps a turn's end in the outbox, an answer for each reader. `key`
/// names what ended among all the resident's ends, as the answers' ids do:
/// its turn, or the words whose turn is not known.
#[allow(clippy::too_many_arguments)]
async fn keep_answers(
    node: &Arc<Node>,
    id: &ResidentId,
    origin: &Origin,
    directory: &ResidentDir,
    key: &str,
    turn: &str,
    end: &TurnEnd,
    readers: &[Address],
) -> std::io::Result<()> {
    let colleague = Address { machine: node.machine(), resident: id.clone() };
    let label = match origin {
        Origin::Colleague { label, .. } => label.clone(),
        Origin::User => String::new(),
    };
    let answers: Vec<Owing> = readers
        .iter()
        .map(|reader| Owing {
            to: reader.clone(),
            delivery: Delivery {
                id: speech::fixed_id("answer", &[&colleague.tag(), key, &reader.tag()]),
                at: Timestamp::now(),
                notice: Notice::Answer {
                    colleague: colleague.clone(),
                    label: label.clone(),
                    turn: turn.to_string(),
                    end: end.clone(),
                },
            },
        })
        .collect();
    let directory = directory.clone();
    tokio::task::spawn_blocking(move || {
        for answer in &answers {
            crate::inbox::owe(&directory, answer)?;
        }
        Ok(())
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Tells each reader the answers kept for it in the resident's outbox; each
/// leaves the outbox once its reader's machine has it, or telling it failed
/// for good, which the colleague's records keep.
pub async fn pay(node: &Arc<Node>, id: &ResidentId, directory: &ResidentDir) {
    let owing = {
        let directory = directory.clone();
        tokio::task::spawn_blocking(move || crate::inbox::owing(&directory)).await
    };
    let owing = match owing {
        Ok(Ok(owing)) => owing,
        Ok(Err(error)) => {
            node.log.line(format_args!("reading the answers {id} owes: {error}"));
            return;
        }
        Err(_) => return,
    };
    let colleague = node.residents.about(id).is_some_and(|(about, _)| matches!(about.origin, Origin::Colleague { .. }));
    for answer in owing {
        let turn = match &answer.delivery.notice {
            Notice::Answer { turn, .. } => Some(turn.clone()).filter(|turn| !turn.is_empty()),
            _ => None,
        };
        // Telling can lead back here, as a reader's turn ends at once.
        let landed = Box::pin(speech::tell(
            node,
            &answer.to,
            answer.delivery.id.clone(),
            answer.delivery.notice.clone(),
            Owed::Working,
        ))
        .await;
        if let Err(failure) = &landed {
            node.log.line(format_args!("telling {} how {id}'s turn ended: {failure}", answer.to));
        }
        if colleague {
            let line = MessageLine {
                at: Timestamp::now(),
                what: MessageKind::Told { to: answer.to.clone(), turn, outcome: speech::outcome(&landed) },
            };
            if let Err(error) = crate::residents::append_message(directory, &line) {
                node.log.line(format_args!("recording that {} was told: {error}", answer.to));
            }
        }
        if let Err(error) = crate::inbox::paid(directory, &answer.delivery.id) {
            node.log.line(format_args!("settling the answer for {}: {error}", answer.to));
        }
    }
}

/// Words kept for a resident whose fate will not be told by any turn's end:
/// they did not go in (`how` failed), or whether they did is not known.
/// Whoever they were for hears that, once, and they are kept no more.
pub async fn unsettled(
    node: &Arc<Node>,
    id: &ResidentId,
    directory: &ResidentDir,
    pending: Pending,
    how: TurnHow,
    why: String,
) {
    let Ok(Some(pending)) = speech::settle(directory, &pending.delivery.id).await else {
        return;
    };
    let Some((about, _)) = node.residents.about(id) else { return };
    let end = TurnEnd { how, reason: Some(why), duration_ms: None, last_message: None };
    let key = pending.delivery.id.to_string();
    if let Err(error) = keep_answers(node, id, &about.origin, directory, &key, "", &end, &pending.readers).await {
        node.log.line(format_args!("keeping what became of words for {id}: {error}"));
        return;
    }
    pay(node, id, directory).await;
}

/// The host reported something about a resident's session itself.
pub fn health(node: &Arc<Node>, id: &ResidentId, turn: Option<String>, report: Health) {
    let at = Timestamp::now();
    let event = SessionEvent { at, happened: Happened::Health { turn, health: report.clone() } };
    node.watchers.publish(id, &event);
    let trouble = Trouble { at, text: report.text.clone() };
    if let Err(error) = node.residents.change_now(&node.world, id, |now| now.trouble = Some(trouble)) {
        node.log.line(format_args!("recording what {id}'s host reported: {error}"));
    }
    if !report.kind.urgent() {
        return;
    }
    let readers = node.residents.first_told(id, report.kind);
    let label = match node.residents.about(id) {
        Some((about, _)) => match about.origin {
            Origin::Colleague { label, .. } => label,
            Origin::User => String::new(),
        },
        None => return,
    };
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move {
        let colleague = Address { machine: node.machine(), resident: id.clone() };
        for reader in readers {
            let notice = Notice::Health { colleague: colleague.clone(), label: label.clone(), health: report.clone() };
            if let Err(failure) = speech::tell(&node, &reader, speech::new_id("health"), notice, Owed::Working).await {
                node.log.line(format_args!("telling {reader} what {id}'s host reported: {failure}"));
            }
        }
    });
}

/// A turn stopped because something stopped under it.
pub async fn stopped(node: &Arc<Node>, id: &ResidentId, turn: &str, reason: String) {
    let end = TurnEnd { how: TurnHow::Interrupted, reason: Some(reason), duration_ms: None, last_message: None };
    ended(node, id, turn, end).await;
}

/// Ends the turns the node stopped under, now that it is up again, and
/// tells whoever was owed them.
pub async fn recover(node: &Arc<Node>) {
    let reason = format!("the node on {} restarted while it ran", node.machine());
    for (id, turn) in node.residents.unfinished() {
        stopped(node, &id, &turn, reason.clone()).await;
    }
}
