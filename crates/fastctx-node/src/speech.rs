//! Words and facts on their way to a resident (`world-colleagues.md`, 话怎样
//! 送到居民眼前).
//!
//! What is for a resident of this machine goes the way its host lets the
//! World in: into the thread of a Codex resident the World holds, where it
//! starts a turn or joins the running one at its next tool boundary;
//! otherwise into its inbox, which the World's tools hand over with their
//! next answer. What is for a resident of another machine goes there as a
//! letter, which that machine hands over the same way when it has it.
//!
//! Words that go into a session make someone owed the end of the turn they
//! go into: whoever said them, or, for a fact of the resident's own work
//! such as a job's end, whoever it is working for.

use std::sync::Arc;

use fastctx_hands::told;
use fastctx_hosts::codex::Input;
use fastctx_net::message::LetterId;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::Letter;
use fastctx_types::residents::{Delivery, Landed, MessageKind, MessageLine, Notice, Origin, Outcome};
use fastctx_types::session::{Happened, Health, HealthKind, SessionEvent, Speaker};
use fastctx_types::{Address, NoticeId, ResidentId};
use jiff::Timestamp;

use crate::Node;

/// Who is owed the end of the turn words go into.
#[derive(Clone, Debug)]
pub enum Owed {
    /// Whoever said them.
    Speaker(Address),
    /// Whoever the resident is working for: those owed its latest turn.
    Working,
    /// Nobody: the user's own words, which the user watches.
    Nobody,
}

/// Tells `to` something. `id` names it, here and in the letter that carries
/// it to another machine; the same id told again is the same notice.
pub async fn tell(node: &Arc<Node>, to: &Address, id: NoticeId, notice: Notice, owed: Owed) -> Result<Landed, Failure> {
    if to.machine == node.machine() {
        return tell_here(node, &to.resident, Delivery { id, at: Timestamp::now(), notice }, owed).await;
    }
    let link = node.link().ok_or_else(|| {
        Failure::new(format!(
            "{} lives on {}, and this machine is in no World with it any more",
            to.resident, to.machine
        ))
    })?;
    let letter = match notice {
        // Words a resident said make it owed the turn they go into, which
        // the receiving machine knows from who sent them.
        Notice::Words { from: Speaker::Resident { address }, text }
            if address.machine == node.machine() && matches!(owed, Owed::Speaker(_)) =>
        {
            Letter::Speak { to: to.resident.clone(), from: address.resident, text }
        }
        notice => Letter::Notice { to: to.resident.clone(), notice },
    };
    let id = LetterId::try_from(id.to_string()).expect("a notice id is a letter id");
    link.send_letter(&to.machine, id, letter)
        .map_err(|error| Failure::new(format!("sending to {}: {error}", to.machine)))?;
    Ok(Landed::Sent)
}

/// Tells a resident of this machine something.
pub async fn tell_here(node: &Arc<Node>, to: &ResidentId, delivery: Delivery, owed: Owed) -> Result<Landed, Failure> {
    let (about, town) = node
        .residents
        .about(to)
        .ok_or_else(|| Failure::new(format!("no resident {to} lives on {}", node.machine())))?;
    if let Some(landed) = crate::claude::deliver(node, to, &about, &delivery, &owed).await {
        return landed;
    }
    let mut why = None;
    if about.world_started
        && about.host.agent.as_str() == "codex"
        && let Some(thread) = &about.session
    {
        match into_thread(node, to, thread, &delivery, owed).await {
            Ok(landed) => return Ok(landed),
            // Kept until the resident can take it, as for any session the
            // World cannot reach now: its next answer of a World tool hands
            // it over. The caller hears why it waits.
            Err(failure) => {
                node.log
                    .line(format_args!("{} could not go to {to} now, and waits in its inbox: {failure}", delivery.id));
                why = Some(failure.message);
            }
        }
    }
    let world = node.world.clone();
    let to = to.clone();
    tokio::task::spawn_blocking(move || crate::inbox::put(&world, town.as_ref(), &to, &delivery))
        .await
        .map_err(|error| Failure::new(format!("keeping it in the inbox: {error}")))?
        .map_err(|error| Failure::new(format!("keeping it in the inbox: {error}")))?;
    Ok(Landed::Waiting { why })
}

/// Puts a notice into the thread of a Codex resident the World holds, and
/// makes who it is for owed the turn it goes into.
async fn into_thread(
    node: &Arc<Node>,
    id: &ResidentId,
    thread: &str,
    delivery: &Delivery,
    owed: Owed,
) -> Result<Landed, Failure> {
    let input = match &delivery.notice {
        // A fact of its own work reads best as the output of the tool that
        // started it.
        notice @ Notice::JobEnded { .. } => {
            Input::ToolOutput { name: "run".to_string(), namespace: None, output: told::told(notice) }
        }
        notice => Input::Text(told::told(notice)),
    };
    // Its tools start before the words go in, outside the lock others' words
    // and its turns' ends wait on.
    let missing = node.codex.prepare(thread).await.map_err(Failure::new)?;
    let speaking = node.residents.speaking(id).ok_or_else(|| Failure::new(format!("{id} has no record here")))?;
    let sent = {
        let _speaking = speaking.lock().await;
        let readers = match &owed {
            Owed::Speaker(address) => vec![address.clone()],
            Owed::Working => node.residents.now(id).map(|now| now.answering).unwrap_or_default(),
            Owed::Nobody => Vec::new(),
        };
        let sent = node.codex.send(thread, input).await.map_err(Failure::new)?;
        let owed = node.residents.owe(&node.world, id, &sent.turn, &readers);
        owed.map_err(|error| Failure::new(format!("recording who is owed the turn: {error}")))?;
        sent
    };
    if let Notice::Words { from, text } = &delivery.notice {
        let event = SessionEvent {
            at: delivery.at,
            happened: Happened::Heard { turn: Some(sent.turn.clone()), from: Some(from.clone()), text: text.clone() },
        };
        node.watchers.publish(id, &event);
        if let Some((about, town)) = node.residents.about(id)
            && matches!(about.origin, Origin::Colleague { .. })
        {
            let line = MessageLine {
                at: delivery.at,
                what: MessageKind::Heard { from: from.clone(), turn: Some(sent.turn.clone()), text: text.clone() },
            };
            let directory = node.world.resident(town.as_ref(), id);
            if let Err(error) = crate::residents::append_message(&directory, &line) {
                node.log.line(format_args!("recording what {id} heard: {error}"));
            }
        }
    }
    if let Some(missing) = missing {
        let health = Health { kind: HealthKind::ToolsMissing, text: missing };
        crate::turns::health(node, id, Some(sent.turn.clone()), health);
    }
    Ok(if sent.joined { Landed::JoinedTurn { turn: sent.turn } } else { Landed::NewTurn { turn: sent.turn } })
}

/// What a told answer came to, for the colleague's `messages`.
pub fn outcome(landed: &Result<Landed, Failure>) -> Outcome {
    match landed {
        Ok(Landed::NewTurn { .. } | Landed::JoinedTurn { .. } | Landed::Handed) => Outcome::Delivered,
        Ok(Landed::Waiting { why }) => Outcome::Waiting { why: why.clone() },
        Ok(Landed::Sent) => Outcome::Sent,
        Err(failure) => Outcome::Failed { reason: failure.message.clone() },
    }
}

/// A new notice id: the time and eight random hex digits.
pub fn new_id(prefix: &str) -> NoticeId {
    let mut bytes = [0u8; 4];
    // With no randomness the time alone still names it.
    let _ = getrandom::fill(&mut bytes);
    let at = Timestamp::now().as_millisecond();
    NoticeId::try_from(format!("{prefix}-{at}-{:08x}", u32::from_be_bytes(bytes))).expect("a word, digits and hex")
}
