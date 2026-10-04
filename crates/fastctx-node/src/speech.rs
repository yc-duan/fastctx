//! Words and facts on their way to a resident (`world-colleagues.md`, 话怎样
//! 送到居民眼前).
//!
//! What is for a resident of this machine is kept first, with who is owed
//! the end of the turn it goes into (`crate::inbox`), and then goes the way
//! its host lets the World in: into the thread of a Codex resident the World
//! holds, where it starts a turn or joins the running one at its next tool
//! boundary; to the World's plugin in a Claude session, which says which
//! turn took it; otherwise it waits, and the World's tools hand it over with
//! their next answer. It stays kept until the World knows which turn took
//! it. What is for a resident of another machine goes there as a letter,
//! which that machine takes the same way.
//!
//! Words that go into a session make someone owed the end of the turn they
//! go into: whoever said them, or, for a fact of the resident's own work
//! such as a job's end, whoever it is working for.

use std::sync::Arc;

use fastctx_hands::told;
use fastctx_hosts::codex::{Input, SendError};
use fastctx_net::message::LetterId;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::ResidentDir;
use fastctx_types::remote::Letter;
use fastctx_types::residents::{
    About, Delivery, Landed, MessageKind, MessageLine, Notice, Origin, Outcome, Pending, PendingState,
};
use fastctx_types::session::{Happened, Health, HealthKind, SessionEvent, Speaker, TurnHow};
use fastctx_types::{Address, NoticeId, ResidentId};
use jiff::Timestamp;

use crate::Node;

/// Who is owed the end of the turn words go into.
#[derive(Clone, Debug)]
pub enum Owed {
    /// Whoever said them.
    Speaker(Address),
    /// Whoever the resident is working for.
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
        notice => Letter::Notice { to: to.resident.clone(), notice: fitted(&link, notice, to) },
    };
    let id = LetterId::try_from(id.to_string()).expect("a notice id is a letter id");
    link.send_letter(&to.machine, id, letter)
        .map_err(|error| Failure::new(format!("sending to {}: {error}", to.machine)))?;
    Ok(Landed::Sent)
}

/// A colleague's answer that would not fit in one letter, cut to what fits
/// and saying where the whole of it is: in the colleague's records on its
/// machine. The size is the letter's limit less what else the letter holds.
fn fitted(link: &fastctx_net::link::Link, notice: Notice, to: &Address) -> Notice {
    let Notice::Answer { colleague, label, turn, mut end } = notice else { return notice };
    let Some(message) = end.last_message.take() else {
        return Notice::Answer { colleague, label, turn, end };
    };
    let letter = |end: &fastctx_types::session::TurnEnd| {
        let notice =
            Notice::Answer { colleague: colleague.clone(), label: label.clone(), turn: turn.clone(), end: end.clone() };
        link.letter_size(&to.machine, &Letter::Notice { to: to.resident.clone(), notice })
    };
    end.last_message = Some(message.clone());
    if letter(&end) <= fastctx_net::wire::MAX_CONTROL_FRAME {
        return Notice::Answer { colleague, label, turn, end };
    }
    let total = message.chars().count();
    let note = |kept: usize| {
        format!(
            "\n… (the answer goes on: it is {total} characters, of which these are the first {kept}; the whole of it is in {}'s records on {}: fastctx w {} shows where)",
            colleague.resident, colleague.machine, colleague.resident
        )
    };
    // The most characters that fit, found by halving: each try is one
    // measure of the letter as it would go.
    let (mut fits, mut over) = (0usize, total);
    while over - fits > 1 {
        let middle = fits + (over - fits) / 2;
        let kept: String = message.chars().take(middle).collect();
        end.last_message = Some(kept + &note(middle));
        if letter(&end) <= fastctx_net::wire::MAX_CONTROL_FRAME { fits = middle } else { over = middle }
    }
    let kept: String = message.chars().take(fits).collect();
    end.last_message = Some(kept + &note(fits));
    Notice::Answer { colleague, label, turn, end }
}

/// Tells a resident of this machine something: kept first, then handed over
/// the way its host lets the World in.
pub async fn tell_here(node: &Arc<Node>, to: &ResidentId, delivery: Delivery, owed: Owed) -> Result<Landed, Failure> {
    let (about, town) = node
        .residents
        .about(to)
        .ok_or_else(|| Failure::new(format!("no resident {to} lives on {}", node.machine())))?;
    let readers = match owed {
        Owed::Speaker(address) => vec![address],
        Owed::Working => node.residents.working_for(to),
        Owed::Nobody => Vec::new(),
    };
    let directory = node.world.resident(town.as_ref(), to);
    let pending = Pending { delivery, readers, state: PendingState::Waiting };
    let kept = {
        let (directory, pending) = (directory.clone(), pending.clone());
        tokio::task::spawn_blocking(move || crate::inbox::keep(&directory, &pending))
            .await
            .map_err(|error| Failure::new(format!("keeping it: {error}")))?
            .map_err(|error| Failure::new(format!("keeping it: {error}")))?
    };
    if !kept {
        // The same thing told again: it is on its way or went in already.
        return Ok(Landed::Waiting { why: None, answered: answered(node, to, &about) });
    }
    hand_over(node, to, &about, &directory, pending).await
}

/// Puts something kept into the resident's session, if its host lets the
/// World in now; otherwise it waits.
pub async fn hand_over(
    node: &Arc<Node>,
    to: &ResidentId,
    about: &About,
    directory: &ResidentDir,
    pending: Pending,
) -> Result<Landed, Failure> {
    if let Some(landed) = crate::claude::deliver(node, to, about, directory, &pending).await {
        return landed;
    }
    let mut why = None;
    if about.world_started
        && about.host.agent.as_str() == "codex"
        && let Some(thread) = &about.session
    {
        match into_thread(node, to, thread, directory, pending).await {
            Ok(landed) => return Ok(landed),
            Err(failure) => {
                node.log.line(format_args!("words could not go to {to} now, and wait: {failure}"));
                why = Some(failure.message);
            }
        }
    }
    Ok(Landed::Waiting { why, answered: answered(node, to, about) })
}

/// Whether the end of the turn words go into will be told: the World sees
/// the turns of the Codex threads it holds and of the Claude sessions its
/// plugin runs in, no others.
fn answered(node: &Node, to: &ResidentId, about: &About) -> bool {
    match about.host.agent.as_str() {
        "codex" => about.world_started,
        "claude" => about.world_started || about.session.is_some() || crate::claude::paired(node, to),
        _ => false,
    }
}

/// Puts what is kept into the thread of a Codex resident the World holds:
/// what waited for a way in first, in order, with `pending`. Whoever each
/// is for is owed the turn they go into.
async fn into_thread(
    node: &Arc<Node>,
    id: &ResidentId,
    thread: &str,
    directory: &ResidentDir,
    pending: Pending,
) -> Result<Landed, Failure> {
    // Its tools start before the words go in, outside the lock others' words
    // and its turns' ends wait on.
    let missing = node.codex.prepare(thread).await.map_err(Failure::new)?;
    let speaking = node.residents.speaking(id).ok_or_else(|| Failure::new(format!("{id} has no record here")))?;
    let _speaking = speaking.lock().await;
    let mut batch: Vec<Pending> = {
        let directory = directory.clone();
        tokio::task::spawn_blocking(move || crate::inbox::pending(&directory))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("reading what waits: {error}")))?
    };
    batch.retain(|kept| kept.state == PendingState::Waiting && kept.delivery.id != pending.delivery.id);
    batch.push(pending);
    let input = match batch.as_slice() {
        // A fact of its own work reads best as the output of the tool that
        // started it.
        [only] if matches!(only.delivery.notice, Notice::JobEnded { .. }) => {
            Input::ToolOutput { name: "run".to_string(), namespace: None, output: told::told(&only.delivery.notice) }
        }
        batch => {
            Input::Text(batch.iter().map(|kept| told::told(&kept.delivery.notice)).collect::<Vec<_>>().join("\n\n"))
        }
    };
    for kept in &batch {
        set_state(directory, &kept.delivery.id, PendingState::Sending).await?;
    }
    let sent = match node.codex.send(thread, input).await {
        Ok(sent) => sent,
        Err(SendError::NotSent(why)) => {
            for kept in &batch {
                set_state(directory, &kept.delivery.id, PendingState::Waiting).await?;
            }
            return Err(Failure::new(why));
        }
        Err(SendError::Unknown(why)) => {
            // Whether they went in is not known, and a turn they went into
            // ended as the app-server stopped: each reader hears that.
            for kept in batch {
                crate::turns::unsettled(node, id, directory, kept, TurnHow::Interrupted, why.clone()).await;
            }
            return Ok(Landed::Waiting { why: Some(why), answered: true });
        }
    };
    let readers: Vec<Address> = batch.iter().flat_map(|kept| kept.readers.iter().cloned()).collect();
    let owed = node.residents.owe(&node.world, id, &sent.turn, &readers);
    owed.map_err(|error| Failure::new(format!("recording who is owed the turn: {error}")))?;
    for kept in &batch {
        settle(directory, &kept.delivery.id).await?;
        heard(node, id, &kept.delivery, &sent.turn);
    }
    if let Some(missing) = missing {
        let health = Health { kind: HealthKind::ToolsMissing, text: missing };
        crate::turns::health(node, id, Some(sent.turn.clone()), health);
    }
    Ok(if sent.joined { Landed::JoinedTurn { turn: sent.turn } } else { Landed::NewTurn { turn: sent.turn } })
}

/// Words went into `turn`: everyone watching sees them, and a colleague's
/// records keep them.
pub fn heard(node: &Arc<Node>, id: &ResidentId, delivery: &Delivery, turn: &str) {
    let Notice::Words { from, text } = &delivery.notice else { return };
    let event = SessionEvent {
        at: delivery.at,
        happened: Happened::Heard { turn: Some(turn.to_string()), from: Some(from.clone()), text: text.clone() },
    };
    node.watchers.publish(id, &event);
    if let Some((about, town)) = node.residents.about(id)
        && matches!(about.origin, Origin::Colleague { .. })
    {
        let line = MessageLine {
            at: delivery.at,
            what: MessageKind::Heard { from: from.clone(), turn: Some(turn.to_string()), text: text.clone() },
        };
        let directory = node.world.resident(town.as_ref(), id);
        if let Err(error) = crate::residents::append_message(&directory, &line) {
            node.log.line(format_args!("recording what {id} heard: {error}"));
        }
    }
}

async fn set_state(directory: &ResidentDir, id: &NoticeId, state: PendingState) -> Result<(), Failure> {
    let (directory, id) = (directory.clone(), id.clone());
    tokio::task::spawn_blocking(move || crate::inbox::set_state(&directory, &id, state))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("keeping what is on its way: {error}")))
}

/// Something kept went in: it is no longer kept, and its id is known.
pub async fn settle(directory: &ResidentDir, id: &NoticeId) -> Result<Option<Pending>, Failure> {
    let (directory, id) = (directory.clone(), id.clone());
    tokio::task::spawn_blocking(move || crate::inbox::settle(&directory, &id))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("settling what went in: {error}")))
}

/// Finishes what a node that stopped left on its way: what was being put
/// into a Codex thread as it stopped went in or not, nobody knows, and its
/// readers hear so; what waited for a Codex thread goes in now; the ends of
/// turns not yet told are told.
pub async fn recover(node: &Arc<Node>) {
    for resident in node.residents.everyone() {
        let directory = node.world.resident(resident.town.as_ref(), &resident.id);
        let kept = match crate::inbox::pending(&directory) {
            Ok(kept) => kept,
            Err(error) => {
                node.log.line(format_args!("reading what is on its way to {}: {error}", resident.id));
                continue;
            }
        };
        let mut waiting = None;
        for pending in kept {
            match pending.state {
                PendingState::Sending => {
                    let why = format!(
                        "the node on {} restarted as they went in; whether {} read them is not known",
                        node.machine(),
                        resident.id
                    );
                    crate::turns::unsettled(node, &resident.id, &directory, pending, TurnHow::Interrupted, why).await;
                }
                PendingState::Waiting
                    if resident.about.world_started && resident.about.host.agent.as_str() == "codex" =>
                {
                    waiting = Some(pending)
                }
                _ => {}
            }
        }
        // One hand-over takes everything waiting, in order.
        if let Some(pending) = waiting
            && let Err(failure) = hand_over(node, &resident.id, &resident.about, &directory, pending).await
        {
            node.log.line(format_args!("handing over what waited for {}: {failure}", resident.id));
        }
        crate::turns::pay(node, &resident.id, &directory).await;
    }
}

/// What a told answer came to, for the colleague's `messages`.
pub fn outcome(landed: &Result<Landed, Failure>) -> Outcome {
    match landed {
        Ok(Landed::NewTurn { .. } | Landed::JoinedTurn { .. } | Landed::Handed) => Outcome::Delivered,
        Ok(Landed::Waiting { why, .. }) => Outcome::Waiting { why: why.clone() },
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

/// A notice id that is the same whenever it is made from the same parts:
/// the same thing told again is known for what it is.
pub fn fixed_id(prefix: &str, parts: &[&str]) -> NoticeId {
    use sha2::{Digest as _, Sha256};
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    let digest: String = hash.finalize().iter().take(10).map(|byte| format!("{byte:02x}")).collect();
    NoticeId::try_from(format!("{prefix}-{digest}")).expect("a word and hex")
}
