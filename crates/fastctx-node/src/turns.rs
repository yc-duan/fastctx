//! How a turn of a session the World holds ends, the one way for all three
//! ways it can (`world-colleagues.md`, 一轮怎样结束): its host reports the
//! end; the host's process stops under it; or the node stops while it runs
//! and finds it unfinished when it starts again. Each ends here: the
//! resident's records say how, everyone watching sees it, and everyone owed
//! the turn's end gets the agent's last message, verbatim.
//!
//! What the host reports about the session itself, rather than its work,
//! goes into the resident's `now` and to everyone watching; what must be
//! heard at once also goes to everyone owed the running turn's end.

use std::sync::Arc;

use fastctx_types::residents::{MessageKind, MessageLine, Notice, Origin, Trouble};
use fastctx_types::session::{Happened, Health, SessionEvent, TurnEnd, TurnHow};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use crate::Node;
use crate::speech::{self, Owed};

/// Something a host reported about a resident's session.
pub async fn happened(node: &Arc<Node>, id: &ResidentId, event: SessionEvent) {
    match event.happened {
        Happened::TurnEnded { turn, end } => ended(node, id, &turn, end).await,
        Happened::Health { turn, health: report } => health(node, id, turn, report),
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
/// was never known: its words did not get into the session.
pub async fn ended(node: &Arc<Node>, id: &ResidentId, turn: &str, end: TurnEnd) {
    let at = Timestamp::now();
    if !turn.is_empty() {
        let event = SessionEvent { at, happened: Happened::TurnEnded { turn: turn.to_string(), end: end.clone() } };
        node.watchers.publish(id, &event);
    }
    let Some((about, town)) = node.residents.about(id) else {
        return;
    };
    let here = node.machine();
    let (requester_here, label) = match &about.origin {
        Origin::Colleague { asked_by, label } => (node.residents.requester_here(asked_by, &here), label.clone()),
        Origin::User => (true, String::new()),
    };
    let trouble = match (&end.how, &end.reason) {
        (TurnHow::Completed, _) => None,
        (how, Some(reason)) => Some(Trouble { at, text: format!("turn {how}: {reason}") }),
        (how, None) => Some(Trouble { at, text: format!("turn {how}") }),
    };
    let readers = {
        let Some(speaking) = node.residents.speaking(id) else { return };
        let _speaking = speaking.lock().await;
        match node.residents.take_owed(&node.world, id, turn, requester_here, trouble) {
            Ok(readers) => readers,
            Err(error) => {
                node.log.line(format_args!("recording how {id}'s turn ended: {error}"));
                return;
            }
        }
    };
    let directory = node.world.resident(town.as_ref(), id);
    let colleague = matches!(about.origin, Origin::Colleague { .. });
    if colleague {
        let line = MessageLine {
            at,
            what: MessageKind::Ended { turn: turn.to_string(), end: end.clone(), owed: readers.clone() },
        };
        if let Err(error) = crate::residents::append_message(&directory, &line) {
            node.log.line(format_args!("recording how {id}'s turn ended: {error}"));
        }
    }
    // Telling may wait for a reader's own session; the turn's end is
    // settled already.
    let (node, id, turn) = (node.clone(), id.clone(), turn.to_string());
    tokio::spawn(async move {
        let colleague_address = Address { machine: node.machine(), resident: id.clone() };
        for reader in readers {
            let notice = Notice::Answer {
                colleague: colleague_address.clone(),
                label: label.clone(),
                turn: turn.clone(),
                end: end.clone(),
            };
            let landed = speech::tell(&node, &reader, speech::new_id("answer"), notice, Owed::Working).await;
            if let Err(failure) = &landed {
                node.log.line(format_args!("telling {reader} how {id}'s turn ended: {failure}"));
            }
            if colleague {
                let line = MessageLine {
                    at: Timestamp::now(),
                    what: MessageKind::Told {
                        to: reader.clone(),
                        turn: Some(turn.clone()).filter(|turn| !turn.is_empty()),
                        outcome: speech::outcome(&landed),
                    },
                };
                if let Err(error) = crate::residents::append_message(&directory, &line) {
                    node.log.line(format_args!("recording that {reader} was told: {error}"));
                }
            }
        }
    });
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
