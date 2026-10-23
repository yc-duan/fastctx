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
use fastctx_types::ipc::Failure;
use fastctx_types::layout::ConversationDir;
use fastctx_types::remote::Letter;
use fastctx_types::residents::{
    About, Delivery, FIRST_WORDS_CHARS, Landed, MessageKind, MessageLine, Notice, Origin, Outcome, Pending,
    PendingState,
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
pub async fn tell(
    node: &Arc<Node>,
    acting: &fastctx_types::access::Acting,
    to: &Address,
    id: NoticeId,
    notice: Notice,
    owed: Owed,
) -> Result<Landed, Failure> {
    tell_at(node, acting, to, &(&to.machine).into(), id, notice, owed).await
}

/// Tells a resident through the hub that answered a location query.
pub(crate) async fn tell_at(
    node: &Arc<Node>,
    acting: &fastctx_types::access::Acting,
    to: &Address,
    machine: &fastctx_types::access::Written,
    id: NoticeId,
    notice: Notice,
    owed: Owed,
) -> Result<Landed, Failure> {
    if node.is_here(machine) {
        let delivery = Delivery { id, at: Timestamp::now(), notice };
        if crate::on_delivery::park(node, &to.resident, &delivery)
            .map_err(|error| Failure::new(format!("keeping the on completion: {error}")))?
        {
            return Ok(Landed::Waiting { why: None, answered: false });
        }
        return tell_here(node, &to.resident, delivery, owed).await;
    }
    if notice.is_question() {
        crate::goals::questions::require_remote(node, acting, machine).await?;
    }
    if matches!(&notice, Notice::Words { goal_report: Some(_), .. }) {
        crate::goals::reports::require_remote(node, acting, machine).await?;
    }
    let letter = match notice {
        // Words a resident said make it owed the turn they go into, which
        // the receiving machine knows from who sent them.
        Notice::Words { from: Speaker::Resident { address }, text, goal_report: None, goal_question: None }
            if node.is_me(&address.machine) && matches!(owed, Owed::Speaker(_)) =>
        {
            Letter::Speak { to: to.resident.clone(), from: address.resident, text, accounts: Vec::new() }
        }
        notice => Letter::Notice { to: to.resident.clone(), notice: Box::new(notice) },
    };
    crate::letters::send(node, acting, machine, id, letter)
        .map_err(|error| Failure::new(format!("sending to {}: {error}", to.machine)))?;
    Ok(Landed::Sent)
}

/// Tells a resident of this machine something: kept first, then handed over
/// the way its host lets the World in.
pub async fn tell_here(node: &Arc<Node>, to: &ResidentId, delivery: Delivery, owed: Owed) -> Result<Landed, Failure> {
    let actions = crate::control_queue::gate(node, to);
    let _action = if delivery.notice.along().is_none() { Some(actions.lock().await) } else { None };
    if _action.is_some() && node.residents.active(to).is_ok() {
        crate::control_queue::drain(node, to).await?;
    }
    let gate = node.residents.management(to)?;
    // Someone's words are asked from outside: one being deleted refuses
    // them at once, and they wait for what changes its life no longer than
    // anything else asked. What comes of work waits as long as it takes.
    let _reading = if matches!(
        delivery.notice,
        Notice::Words { goal_report: None, goal_question: None, .. } | Notice::Attached { .. }
    ) {
        if node.residents.is_deleting(to) {
            return Err(Failure::new(format!("{to} is being deleted")));
        }
        crate::gate_waits::shared(node, to, &gate).await?
    } else {
        gate.read().await
    };
    tell_here_locked(node, to, delivery, owed).await
}

pub(crate) async fn tell_here_locked(
    node: &Arc<Node>,
    to: &ResidentId,
    delivery: Delivery,
    owed: Owed,
) -> Result<Landed, Failure> {
    if crate::on_delivery::park(node, to, &delivery)
        .map_err(|error| Failure::new(format!("keeping the completion: {error}")))?
    {
        return Ok(Landed::Waiting { why: None, answered: false });
    }
    // What still comes for one removed or being deleted, as the end of a
    // command its deletion stopped, is kept with its records and wakes it
    // no more; words for it are refused.
    let deleting = node.residents.is_deleting(to);
    let removed = deleting || node.residents.removed(to).is_some();
    if !removed
        || matches!(
            delivery.notice,
            Notice::Words { goal_report: None, goal_question: None, .. } | Notice::Attached { .. }
        )
    {
        node.residents.active(to)?;
    }
    let (about, _) = node
        .residents
        .about(to)
        .ok_or_else(|| Failure::new(format!("no resident {to} lives on {}", node.machine())))?;
    let working = matches!(owed, Owed::Working);
    let readers = match owed {
        Owed::Speaker(address) => vec![address],
        Owed::Working => Vec::new(),
        Owed::Nobody => Vec::new(),
    };
    let directory = node.world.conversation(to);
    if let Some(kind) = delivery.notice.along() {
        // Told again before it went in: what has not gone anywhere yet goes
        // no more; what a mod holds, it drops (`claude::into_inbox`).
        let earlier = {
            let directory = directory.clone();
            tokio::task::spawn_blocking(move || crate::inbox::pending(&directory))
                .await
                .map_err(|error| Failure::new(error.to_string()))?
                .map_err(|error| Failure::new(format!("reading what waits: {error}")))?
        };
        for kept in earlier {
            if kept.state == PendingState::Waiting
                && kept.delivery.id != delivery.id
                && kept.delivery.notice.along() == Some(kind)
                && kept.delivery.at <= delivery.at
            {
                crate::inbox::withdraw(&directory, &kept.delivery.id)
                    .map_err(|error| Failure::new(format!("replacing what waits: {error}")))?;
            }
        }
    }
    let pending = Pending { delivery, readers, state: PendingState::Waiting, working, session: None, since: None };
    let kept = {
        let (directory, pending) = (directory.clone(), pending.clone());
        tokio::task::spawn_blocking(move || crate::inbox::keep(&directory, &pending))
            .await
            .map_err(|error| Failure::new(format!("keeping it: {error}")))?
            .map_err(|error| Failure::new(format!("keeping it: {error}")))?
    };
    if kept {
        crate::goals::engine::input_changed(node, to);
    }
    if removed {
        let why = if deleting {
            "the resident is being deleted, and this with it"
        } else {
            "the resident was removed; this fact is preserved in its archived inbox and will not wake it"
        };
        return Ok(Landed::Waiting { why: Some(why.into()), answered: false });
    }
    if !kept {
        if let Some(why) = crate::delivery::input_uncertainty(&directory, &pending.delivery.id)
            .map_err(|error| Failure::new(error.to_string()))?
        {
            return Ok(Landed::Uncertain { why });
        }
        if matches!(pending.delivery.notice,Notice::Sentinel {..}) && let Some(current)=crate::inbox::read(&directory,&pending.delivery.id).map_err(|e|Failure::new(e.to_string()))?
            && current.state==PendingState::Waiting {
            return hand_over_locked(node,to,&about,&directory,current).await;
        }
        // The same thing told again: it is on its way or went in already.
        return Ok(Landed::Waiting { why: None, answered: answered(node, to, &about) });
    }
    // What rides with the next turn waits while none runs; a Claude
    // session's mod holds it for the next one (`along`).
    if pending.delivery.notice.along().is_some() {
        return hand_over_locked(node, to, &about, &directory, pending).await;
    }
    // The end of the resident's own job waits for whatever call is waiting
    // now; words for it end the wait (`world-run.md`).
    if !matches!(pending.delivery.notice, Notice::JobEnded { .. }) {
        node.words.raise(&fastctx_types::Address { machine: node.machine(), resident: to.clone() });
    }
    hand_over_locked(node, to, &about, &directory, pending).await
}

/// A turn of `id` began: what waited to ride with the next turn goes into
/// it now (`Notice::along`).
pub async fn along(node: &Arc<Node>, id: &ResidentId) {
    let Some((about, _)) = node.residents.about(id) else { return };
    let directory = node.world.conversation(id);
    let waiting = {
        let directory = directory.clone();
        match tokio::task::spawn_blocking(move || crate::inbox::pending(&directory)).await {
            Ok(Ok(waiting)) => waiting,
            Ok(Err(error)) => {
                node.log.line(format_args!("reading what waits for {id}: {error}"));
                return;
            }
            Err(_) => return,
        }
    };
    for pending in
        waiting.into_iter().filter(|kept| kept.state == PendingState::Waiting && kept.delivery.notice.along().is_some())
    {
        if let Err(failure) = hand_over(node, id, &about, &directory, pending).await {
            node.log.line(format_args!("what rides with {id}'s turn waits: {}", failure.message));
        }
    }
}

/// Puts something kept into the resident's session, if its host lets the
/// World in now; otherwise it waits.
pub async fn hand_over(
    node: &Arc<Node>,
    to: &ResidentId,
    about: &About,
    directory: &ConversationDir,
    pending: Pending,
) -> Result<Landed, Failure> {
    let _doing = fastctx_platform::doing::doing(format!("handing {} to {to}", pending.delivery.id));
    // Startup's recovery may need the gate alone, as to finish a deletion:
    // nothing waits for it holding the gate.
    node.ready().await;
    let actions = crate::control_queue::gate(node, to);
    let _action = actions.lock().await;
    crate::control_queue::drain(node, to).await?;
    let gate = node.residents.management(to)?;
    let _reading = gate.read().await;
    hand_over_locked(node, to, about, directory, pending).await
}

async fn hand_over_locked(
    node: &Arc<Node>,
    to: &ResidentId,
    about: &About,
    directory: &ConversationDir,
    pending: Pending,
) -> Result<Landed, Failure> {
    // Background delivery can load a thread before it submits a turn. Keep
    // that opening and its durable outcome inside the handover boundary too.
    let Some(_answering) = node.handoff.call() else {
        return Ok(Landed::Waiting { why: Some(crate::handoff::WAITING.into()), answered: answered(node, to, about) });
    };
    node.ready().await;
    node.residents.active(to)?;
    if matches!(pending.delivery.notice,Notice::Sentinel {..}) && crate::inbox::read(directory,&pending.delivery.id)
        .map_err(|e|Failure::new(e.to_string()))?.is_none_or(|current|current.state != PendingState::Waiting) {
        return Ok(Landed::Waiting {why:None,answered:answered(node,to,about)});
    }
    if pending.delivery.notice.along().is_some()
        && about.host.agent.as_str() != "claude"
        && node.residents.now(to).is_none_or(|now| now.turn.is_none())
    {
        return Ok(Landed::Waiting { why: None, answered: true });
    }
    if let Err(error) = crate::homes::available(node, about)
        .and_then(|()| crate::assists::may_turn(node, to, about))
        .and_then(|()| crate::town::access::current(node, to))
    {
        // What a closed conversation is woken for is tried again later.
        if pending.delivery.notice.wakes_closed() {
            again_later(node, to, about, directory, pending);
        }
        return Ok(Landed::Waiting { why: Some(error.message), answered: answered(node, to, about) });
    }
    crate::goals::colleagues::incoming(node, to, about, &pending).await?;
    crate::focus::refresh(node, to).await?;
    if let Some(landed) = crate::claude::deliver(node, to, about, directory, &pending).await {
        return landed;
    }
    let mut why = None;
    let sentinel = matches!(pending.delivery.notice, Notice::Sentinel { .. });
    let requested = pending.delivery.notice.wakes_closed();
    if about.host.agent.as_str() == "codex"
        && (about.world_managed() || pending.delivery.notice.wakes_closed())
        && let Some(thread) = &about.session
    {
        if !about.world_started && !crate::codex::native::requested(node, about)? {
            return Ok(Landed::Waiting {
                why: Some("the native host still runs; this message waits at its next World tool boundary".into()),
                answered: false,
            });
        }
        match into_thread(node, to, thread, directory, pending.clone()).await {
            Ok(landed) => {
                if sentinel {
                    crate::sentinels::report(node, &format!("delivery {to}"), None);
                }
                return Ok(landed);
            }
            Err(failure) => {
                if sentinel {
                    crate::sentinels::report(node, &format!("delivery {to}"), Some(failure.message.clone()));
                } else {
                    node.log.line(format_args!("words could not go to {to} now, and wait: {failure}"));
                }
                why = Some(failure.message);
                if requested {
                    again_later(node, to, about, directory, pending);
                }
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
        "codex" => about.world_managed(),
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
    directory: &ConversationDir,
    pending: Pending,
) -> Result<Landed, Failure> {
    submit_thread(node, id, thread, directory, pending, false, false)
        .await
        .map(|landed| landed.expect("ordinary submissions do not defer to a composer"))
}

/// Submits exactly one composer message. Automatic release rechecks idle while holding the speech lock.
pub async fn send_composer(
    node: &Arc<Node>,
    id: &ResidentId,
    delivery: Delivery,
    idle_only: bool,
) -> Result<Option<Landed>, Failure> {
    node.ready().await;
    let actions = crate::control_queue::gate(node, id);
    let _action = actions.lock().await;
    crate::control_queue::drain(node, id).await?;
    let gate = node.residents.management(id)?;
    let _reading = gate.read().await;
    node.residents.active(id)?;
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new("unknown resident"))?;
    let directory = node.world.conversation(id);
    if about.world_managed()
        && about.host.agent.as_str() == "codex"
        && let Some(thread) = &about.session
        && (about.world_started || crate::codex::native::requested(node, &about)?)
    {
        let Some(_answering) = node.handoff.call() else {
            return Err(Failure::new(crate::handoff::WAITING));
        };
        let pending = Pending {
            delivery,
            readers: Vec::new(),
            state: PendingState::Waiting,
            working: false,
            session: None,
            since: None,
        };
        return submit_thread(node, id, thread, &directory, pending, true, idle_only).await;
    }
    if idle_only && node.residents.now(id).is_some_and(|now| now.activity == fastctx_types::residents::Activity::Busy) {
        return Ok(None);
    }
    tell_here_locked(node, id, delivery, Owed::Nobody).await.map(Some)
}

async fn submit_thread(
    node: &Arc<Node>,
    id: &ResidentId,
    thread: &str,
    directory: &ConversationDir,
    pending: Pending,
    single: bool,
    idle_only: bool,
) -> Result<Option<Landed>, Failure> {
    // Its home's app-server holds its thread.
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new(format!("{id} has no record here")))?;
    crate::homes::available(node, &about)?;
    crate::assists::may_turn(node, id, &about)?;
    crate::town::access::current(node, id)?;
    let codex = node.codex.of(node, &about)?;
    // Its tools start before the words go in, outside the lock others' words
    // and its turns' ends wait on.
    let missing = codex.prepare(thread).await.map_err(Failure::new)?;
    let speaking = node.residents.speaking(id).ok_or_else(|| Failure::new(format!("{id} has no record here")))?;
    let _speaking = speaking.lock().await;
    if single
        && idle_only
        && (node.residents.now(id).is_some_and(|now| now.activity == fastctx_types::residents::Activity::Busy)
            || codex.running_turn(thread).is_some())
    {
        return Ok(None);
    }
    // A node handing the World on starts no turn; the words wait for its
    // successor, which hands them over when it serves.
    let Some(_starting) = node.handoff.start_turn() else {
        if single {
            return Err(Failure::new(crate::handoff::Handoff::waiting_why(node)));
        }
        return Ok(Some(Landed::Waiting { why: Some(crate::handoff::Handoff::waiting_why(node)), answered: true }));
    };
    if single && crate::inbox::keep(directory, &pending).map_err(|error| Failure::new(error.to_string()))? {
        crate::goals::engine::input_changed(node, id);
    }
    let accepted: std::collections::HashSet<_> = crate::delivery::handovers(directory)
        .map_err(|error| Failure::new(error.to_string()))?
        .into_iter()
        .filter(|handover| handover.turn.is_some() || handover.unresolved.is_some())
        .flat_map(|handover| handover.inputs.into_iter().map(|input| input.delivery.id))
        .collect();
    let mut batch: Vec<Pending> = {
        let directory = directory.clone();
        tokio::task::spawn_blocking(move || crate::inbox::pending(&directory))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("reading what waits: {error}")))?
    };
    {
        let taking = node.taking.lock().expect("never poisoned");
        batch.retain(|kept| {
            kept.state == PendingState::Waiting
                && !accepted.contains(&kept.delivery.id)
                && !taking.contains(&(id.clone(), kept.delivery.id.clone()))
        });
    }
    if !batch.iter().any(|kept| kept.delivery.id == pending.delivery.id) {
        if let Some(why) = crate::delivery::input_uncertainty(directory, &pending.delivery.id)
            .map_err(|error| Failure::new(error.to_string()))?
        {
            return Ok(Some(Landed::Uncertain { why }));
        }
        return match crate::delivery::input_turn(directory, &pending.delivery.id)
            .map_err(|error| Failure::new(error.to_string()))?
        {
            Some(turn) => Ok(Some(Landed::JoinedTurn { turn })),
            None => Ok(Some(Landed::Waiting {
                why: Some("this delivery is already recorded; it was not submitted again".to_string()),
                answered: true,
            })),
        };
    }
    // The user's words go alone, but for what rides with the next turn.
    if single {
        batch.retain(|kept| kept.delivery.id == pending.delivery.id || kept.delivery.notice.along().is_some());
    }
    // The user's words go in as typed and first: the World's telling after
    // them begins with a head note, so none of it reads as theirs, while
    // their words after it would read as its end. Other words they typed go
    // in the next message.
    let first = if typed(&pending.delivery.notice) {
        Some(pending.delivery.id.clone())
    } else {
        batch.iter().find(|kept| typed(&kept.delivery.notice)).map(|kept| kept.delivery.id.clone())
    };
    let (mut taken, left): (Vec<Pending>, Vec<Pending>) =
        batch.into_iter().partition(|kept| !typed(&kept.delivery.notice) || Some(&kept.delivery.id) == first.as_ref());
    if let Some(at) = taken.iter().position(|kept| Some(&kept.delivery.id) == first.as_ref()) {
        let words = taken.remove(at);
        taken.insert(0, words);
    }
    let mut batch = taken;
    let audience = if batch.iter().any(|pending| pending.working)
        && crate::goals::replies::inherits(node, id).map_err(|error| Failure::new(error.to_string()))?
    {
        node.residents.working_for(id)
    } else {
        Vec::new()
    };
    for kept in &mut batch {
        if kept.working {
            kept.readers = audience.clone();
            kept.working = false;
        }
    }
    let (current, _) = node.residents.about(id).ok_or_else(crate::authorization::no_subject)?;
    crate::homes::available(node, &current)?;
    crate::assists::may_turn(node, id, &current)?;
    let mut handover =
        crate::delivery::begin(directory, batch.clone()).map_err(|error| Failure::new(error.to_string()))?;
    let client = Some(format!("{}{}", fastctx_hosts::codex::WORLD_CLIENT, handover.id));
    let (input, client) = match batch.as_slice() {
        // A fact of its own work reads best as the output of the tool that
        // started it.
        [only] if matches!(only.delivery.notice, Notice::JobEnded { .. }) => (
            Input::ToolOutput { name: "run".to_string(), namespace: None, output: told::told(&only.delivery.notice) },
            None,
        ),
        batch if batch.iter().any(|kept| matches!(kept.delivery.notice, Notice::Attached { .. })) => (
            Input::Messages(
                batch
                    .iter()
                    .map(|kept| {
                        let images = match &kept.delivery.notice {
                            Notice::Attached { message, .. } => message
                                .attachments
                                .iter()
                                .filter(|attachment| attachment.is_image() && node.is_me(&attachment.owner.machine))
                                .map(|attachment| attachment.path.clone())
                                .collect(),
                            _ => Vec::new(),
                        };
                        fastctx_hosts::codex::InputMessage { text: told::told(&kept.delivery.notice), images }
                    })
                    .collect(),
            ),
            client,
        ),
        batch => {
            (Input::Text(batch.iter().map(|kept| said(&kept.delivery.notice)).collect::<Vec<_>>().join("\n\n")), client)
        }
    };
    for kept in &batch {
        set_state(directory, &kept.delivery.id, PendingState::Sending).await?;
    }
    let along_only = batch.iter().all(|input| input.delivery.notice.along().is_some());
    let sending = if along_only {
        let Input::Text(text) = input else { unreachable!("non-waking notices are text facts") };
        codex.send_to_running(thread, text, client).await
    } else {
        codex.send(thread, input, client).await.map(Some)
    };
    let sent = match sending {
        Ok(Some(sent)) => sent,
        Ok(None) => {
            crate::delivery::refused(directory, &mut handover).map_err(|error| Failure::new(error.to_string()))?;
            for kept in &batch {
                set_state(directory, &kept.delivery.id, PendingState::Waiting).await?;
            }
            return Ok(Some(Landed::Waiting { why: None, answered: true }));
        }
        Err(SendError::NotSent(why)) => {
            crate::delivery::refused(directory, &mut handover).map_err(|error| Failure::new(error.to_string()))?;
            for kept in &batch {
                set_state(directory, &kept.delivery.id, PendingState::Waiting).await?;
            }
            if single {
                crate::inbox::withdraw(directory, &pending.delivery.id)
                    .map_err(|error| Failure::new(error.to_string()))?;
            } else {
                again_later(node, id, &about, directory, pending.clone());
            }
            if along_only {
                return Ok(Some(Landed::Waiting { why: Some(why), answered: true }));
            }
            return Err(Failure::new(why));
        }
        Err(SendError::Unknown(why)) => {
            // Whether they went in is not known, and a turn they went into
            // ended as the app-server stopped: each reader hears that.
            crate::turns::unresolved(node, id, directory, &mut handover, TurnHow::Failed, why.clone(), None)
                .await
                .map_err(|error| Failure::new(error.to_string()))?;
            return Ok(Some(Landed::Uncertain { why }));
        }
    };
    if let Err(error) = bind(node, id, directory, &mut handover, &sent.turn).await {
        let accepted = crate::delivery::input_turn(directory, &pending.delivery.id)
            .map_err(|error| Failure::new(error.to_string()))?;
        if accepted.as_deref() != Some(&sent.turn) {
            return Err(error);
        }
        node.log.line(format_args!(
            "{} accepted the words in {}; its durable receipt awaits reconciliation: {error}",
            id, sent.turn
        ));
    }
    if let Some(missing) = missing {
        let health = Health { kind: HealthKind::ToolsMissing, text: missing };
        crate::turns::health(node, id, Some(sent.turn.clone()), health)
            .map_err(|error| Failure::new(error.to_string()))?;
    }
    AGAIN.lock().expect("never poisoned").remove(id);
    // What waited and could not go in with these goes in next, as they would
    // have, rather than waiting for something else to wake it.
    if let Some(next) = left.into_iter().next() {
        hand_over_later(node.clone(), id.clone(), about, directory.clone(), next);
    }
    Ok(Some(if sent.joined { Landed::JoinedTurn { turn: sent.turn } } else { Landed::NewTurn { turn: sent.turn } }))
}

/// How many times words Codex did not take have been offered again, for each
/// resident, since it last took any.
static AGAIN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<ResidentId, u32>>> =
    std::sync::LazyLock::new(Default::default);
/// How many times they are offered again before they wait for the next
/// words, or the node's next start, to take them in.
const AGAIN_TRIES: u32 = 5;

/// Words Codex did not take go in at its next chance, not only with the next
/// words said (`specs/world-concurrency.md`, 一句话没交出去，下一个机会就交):
/// they are offered again after a while, longer each time it still does not
/// take them.
fn again_later(node: &Arc<Node>, id: &ResidentId, about: &About, directory: &ConversationDir, pending: Pending) {
    let tries = {
        let mut again = AGAIN.lock().expect("never poisoned");
        let tries = again.entry(id.clone()).or_insert(0);
        if *tries >= AGAIN_TRIES && !matches!(pending.delivery.notice,Notice::Sentinel {..}) {
            return;
        }
        *tries = tries.saturating_add(1);
        *tries
    };
    let delay = std::time::Duration::from_secs(if matches!(pending.delivery.notice,Notice::Sentinel {..}) {
        (5 * 2u64.pow((tries - 1).min(4))).min(60)
    } else {5 * 3u64.pow(tries - 1)});
    let (node, id, about, directory) = (node.clone(), id.clone(), about.clone(), directory.clone());
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        if let Err(failure) = hand_over(&node, &id, &about, &directory, pending).await {
            node.log.line(format_args!("words for {id} wait still: {}", failure.message));
        }
    });
}

/// Hands `pending` over in a task of its own.
fn hand_over_later(node: Arc<Node>, id: ResidentId, about: About, directory: ConversationDir, pending: Pending) {
    tokio::spawn(async move {
        if let Err(failure) = hand_over(&node, &id, &about, &directory, pending).await {
            node.log.line(format_args!("words for {id} wait: {}", failure.message));
        }
    });
}

/// A notice as it goes into a thread: the user's words as they typed them,
/// everything else as the World tells it.
fn said(notice: &Notice) -> String {
    match notice {
        Notice::Words { from: Speaker::User { .. }, text, .. } => text.clone(),
        notice => told::told(notice),
    }
}

/// Whether a notice is the user's words, which go in as typed.
fn typed(notice: &Notice) -> bool {
    matches!(notice, Notice::Words { from: Speaker::User { .. }, .. })
}

/// Commits the consuming turn for the entire batch before changing either
/// its inbox entries or the turn's debt. The caller holds the speech lock.
pub async fn bind(
    node: &Arc<Node>,
    id: &ResidentId,
    directory: &ConversationDir,
    handover: &mut crate::delivery::Handover,
    turn: &str,
) -> Result<(), Failure> {
    crate::delivery::accept(directory, handover, turn).map_err(|error| Failure::new(error.to_string()))?;
    let readers: Vec<Address> = handover.inputs.iter().flat_map(|input| input.readers.iter().cloned()).collect();
    if let Some(finished) =
        crate::delivery::terminal(directory, turn).map_err(|error| Failure::new(error.to_string()))?
    {
        crate::turns::finish_locked(node, id, turn, finished.end, &readers)
            .await
            .map_err(|error| Failure::new(error.to_string()))?;
    } else {
        let owner = crate::delivery::session(directory, turn).map_err(|error| Failure::new(error.to_string()))?;
        let active = owner.is_none() || node.residents.about(id).is_some_and(|(about, _)| about.session == owner);
        node.residents.owe(&node.world, id, turn, &readers, active).map_err(|error| Failure::new(error.to_string()))?;
    }
    // A stop asked for while these words were on their way stops the turn
    // they started, now that it is known.
    if let Some(source) = node.stops.started(id, handover.inputs.iter().map(|input| &input.delivery.id)) {
        crate::turn_evidence::remember(node, id, turn, source).map_err(|error| Failure::new(error.to_string()))?;
        let (node, id, turn) = (node.clone(), id.clone(), turn.to_string());
        tokio::spawn(async move {
            if node.residents.now(&id).and_then(|now| now.turn).as_deref() != Some(&turn) {
                return;
            }
            if let Err(failure) = crate::talk::stop_here_as(&node, &id, source).await {
                node.log.line(format_args!(
                    "stopping {id}'s turn, asked while its words were on their way: {}",
                    failure.message
                ));
            }
        });
    }
    for input in &handover.inputs {
        crate::composer::bound(node, id, &input.delivery.id, turn).await?;
        heard(node, id, &input.delivery, turn);
        settle(node, directory, &input.delivery.id).await?;
    }
    Ok(())
}

/// What went into `turn`: everyone watching sees who said it, and a
/// colleague's records keep the words said to it. Words are their
/// speaker's; a colleague's answer is that colleague's, as the resident read
/// it; the rest is the World's telling, also as read.
pub fn heard(node: &Arc<Node>, id: &ResidentId, delivery: &Delivery, turn: &str) {
    let (from, text) = match &delivery.notice {
        Notice::Words { from, text, .. } => (from.clone(), text.clone()),
        Notice::Attached { message, text } => (message.speaker(), text.clone()),
        Notice::Answer { colleague, .. } | Notice::Question { colleague, .. } => {
            (Speaker::Resident { address: colleague.clone() }, told::told(&delivery.notice))
        }
        Notice::JobEnded { .. } | Notice::Health { .. } | Notice::Sentinel { .. } => (Speaker::World, told::told(&delivery.notice)),
        Notice::World { text, .. } => (Speaker::World, text.clone()),
    };
    // What an interface shows of the World's telling and a colleague's
    // answer is the notice itself, not the words written for the model.
    let notice = match &delivery.notice {
        Notice::Words { goal_report: Some(_), .. } | Notice::Words { goal_question: Some(_), .. } => {
            Some(Box::new(delivery.notice.clone()))
        }
        Notice::Words { .. } | Notice::Attached { .. } => None,
        Notice::JobEnded { .. }
        | Notice::Answer { .. }
        | Notice::Health { .. }
        | Notice::Question { .. }
        | Notice::World { .. }
        | Notice::Sentinel { .. } => Some(Box::new(delivery.notice.clone())),
    };
    let event = SessionEvent {
        at: Timestamp::now(),
        happened: Happened::Heard {
            message: match &delivery.notice {
                Notice::Attached { message, text } => Some(Box::new(message.message(text))),
                _ => None,
            },
            delivery: Some(delivery.id.clone()),
            turn: Some(turn.to_string()),
            from: Some(from.clone()),
            text: text.clone(),
            notice,
        },
    };
    if !node.residents.knows(id) {
        return;
    }
    let directory = node.world.conversation(id);
    match crate::delivery::event(&directory, &format!("heard:{}", delivery.id), &event) {
        Ok(true) => node.watchers.publish(id, &event),
        Ok(false) => return,
        Err(error) => {
            node.log.line(format_args!("recording what {id} heard: {error}"));
            return;
        }
    }
    // The user's first words name the conversation where its host does not.
    if matches!(from, Speaker::User { .. }) {
        let words = crate::turns::opening(&text, FIRST_WORDS_CHARS);
        if !words.is_empty()
            && let Err(error) = node.residents.change_now(&node.world, id, |now| {
                now.first_words.get_or_insert(words);
            })
        {
            node.log.line(format_args!("recording {id}'s first words: {error}"));
        }
        if let Err(error) = crate::asked::settle(node, id, |asked| crate::asked::answers(&text, asked)) {
            node.log.line(format_args!("recording that {id}'s question was answered: {error}"));
        }
    }
    if let Some((about, _)) = node.residents.about(id)
        && matches!(about.origin, Origin::Colleague { .. })
        && matches!(delivery.notice, Notice::Words { .. })
        && !delivery.notice.is_question()
    {
        let line = MessageLine {
            at: delivery.at,
            what: MessageKind::Heard { from: from.clone(), turn: Some(turn.to_string()), text: text.clone() },
        };
        let directory = node.world.conversation(id);
        if let Err(error) = crate::residents::append_message(&directory, &line) {
            node.log.line(format_args!("recording what {id} heard: {error}"));
        }
    }
}

async fn set_state(directory: &ConversationDir, id: &NoticeId, state: PendingState) -> Result<(), Failure> {
    let (directory, id) = (directory.clone(), id.clone());
    tokio::task::spawn_blocking(move || crate::inbox::set_state(&directory, &id, state))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("keeping what is on its way: {error}")))
}

/// Something kept went in: it is no longer kept, and its id is known.
pub async fn settle(node: &Arc<Node>, directory: &ConversationDir, id: &NoticeId) -> Result<Option<Pending>, Failure> {
    let (directory, id) = (directory.clone(), id.clone());
    let node = node.clone();
    tokio::task::spawn_blocking(move || crate::inbox::settle(&node, &directory, &id))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("settling what went in: {error}")))
}

/// Finishes what a node that stopped left on its way: what was being put
/// into a Codex thread as it stopped went in or not, nobody knows, and its
/// readers hear so; what waited for a Codex thread goes in now; the ends of
/// turns not yet told are told.
pub async fn restore(node: &Arc<Node>) {
    for resident in node.residents.everyone() {
        if resident.removed.is_some() {
            continue;
        }
        let directory = node.world.conversation(&resident.id);
        let result = async {
            // An older node may have saved only part of a terminal outbox.
            // Its saved outcome remains authoritative for every reader.
            for owing in crate::inbox::owing(&directory)? {
                if let Notice::Answer { turn, end, .. } = owing.delivery.notice
                    && !turn.is_empty()
                {
                    let mut readers = node.residents.readers(&resident.id, &turn);
                    readers.push(owing.to);
                    crate::delivery::finish(&directory, &turn, end, &readers)?;
                }
            }
            // Deleted since it was listed: nothing of it is left to restore.
            let Some(speaking) = node.residents.speaking(&resident.id) else { return Ok(()) };
            let _speaking = speaking.lock().await;
            for mut handover in crate::delivery::handovers(&directory)? {
                if let Some(turn) = handover.turn.clone() {
                    bind(node, &resident.id, &directory, &mut handover, &turn)
                        .await
                        .map_err(|error| std::io::Error::other(error.message))?;
                }
            }
            for finished in crate::delivery::terminals(&directory)? {
                crate::turns::finish_locked(node, &resident.id, &finished.turn, finished.end, &finished.readers)
                    .await?;
            }
            Ok::<(), std::io::Error>(())
        }
        .await;
        if let Err(error) = result {
            node.log.line(format_args!("restoring delivery records for {}: {error}", resident.id));
        }
    }
}

/// Classifies interrupted submissions before new submissions are enabled.
/// Delivery itself waits for the node to open, in separate tasks.
pub async fn recover(node: &Arc<Node>) {
    for resident in node.residents.everyone() {
        let directory = node.world.conversation(&resident.id);
        let result = async {
            if resident.removed.is_some() {
                return Ok(None);
            }
            let Some(speaking) = node.residents.speaking(&resident.id) else { return Ok(None) };
            let _guard = speaking.lock().await;
            let handovers = crate::delivery::handovers(&directory)?;
            let covered: std::collections::BTreeSet<NoticeId> = handovers
                .iter()
                .flat_map(|handover| handover.inputs.iter().map(|input| input.delivery.id.clone()))
                .collect();
            for mut handover in handovers {
                if handover.turn.is_none() && (handover.attempting || handover.unresolved.is_some()) {
                    let why = format!(
                        "the node on {} restarted during a submission; whether {} read it is unknown; \
                         its exact contents remain in its handovers record",
                        node.machine(),
                        resident.id
                    );
                    crate::turns::unresolved(
                        node,
                        &resident.id,
                        &directory,
                        &mut handover,
                        TurnHow::Interrupted,
                        why,
                        Some(serde_json::json!({"world":"node_restarted"})),
                    )
                    .await?;
                }
            }
            // Sending entries from versions without batch records are
            // retained and settled as one legacy attempt per resident.
            let kept = crate::inbox::pending(&directory)?;
            let legacy: Vec<Pending> = kept
                .iter()
                .filter(|input| input.state == PendingState::Sending && !covered.contains(&input.delivery.id))
                .cloned()
                .collect();
            if !legacy.is_empty() {
                let mut handover = crate::delivery::begin(&directory, legacy)?;
                crate::turns::unresolved(
                    node,
                    &resident.id,
                    &directory,
                    &mut handover,
                    TurnHow::Interrupted,
                    "the prior node stopped during a submission; acceptance was not recorded".to_string(),
                    Some(serde_json::json!({"world":"node_restarted"})),
                )
                .await?;
            }
            // What can open a turn first, a sentinel's finding before the rest.
            let waiting = crate::inbox::pending(&directory)?
                .into_iter()
                .filter(|input| input.state == PendingState::Waiting)
                .min_by_key(|input| {
                    (input.delivery.notice.along().is_some(), !matches!(input.delivery.notice, Notice::Sentinel { .. }))
                });
            Ok::<_, std::io::Error>(waiting)
        }
        .await;
        match result {
            Err(error) => node.log.line(format_args!(
                "recovering deliveries for {}; records retained for the next start: {error}",
                resident.id
            )),
            Ok(waiting) => {
                let node = node.clone();
                tokio::spawn(async move {
                    if resident.about.host.agent.as_str() == "codex"
                        && let Some(input) = waiting
                        && let Err(error) = hand_over(&node, &resident.id, &resident.about, &directory, input).await
                    {
                        node.log.line(format_args!("handing over recovered words for {}: {error}", resident.id));
                    }
                    crate::turns::pay(&node, &resident.id, &directory).await;
                });
            }
        }
    }
}

/// A cancelled handoff reopens delivery of waiting words. Unlike startup
/// recovery, this never treats an in-flight submission as interrupted.
pub fn resume(node: &Arc<Node>, runtime: &tokio::runtime::Handle) {
    for resident in node.residents.everyone() {
        if resident.removed.is_some() {
            continue;
        }
        if !resident.about.world_managed() || resident.about.host.agent.as_str() != "codex" {
            continue;
        }
        let node = node.clone();
        runtime.spawn(async move {
            let Some(_answering) = node.handoff.call() else { return };
            let directory = node.world.conversation(&resident.id);
            let result = async {
                let Some(speaking) = node.residents.speaking(&resident.id) else { return Ok(()) };
                let guard = speaking.lock().await;
                let waiting = crate::inbox::pending(&directory)?
                    .into_iter()
                    .filter(|input| input.state == PendingState::Waiting)
                    .min_by_key(|input| input.delivery.notice.along().is_some());
                drop(guard);
                if let Some(input) = waiting {
                    hand_over(&node, &resident.id, &resident.about, &directory, input)
                        .await
                        .map_err(|error| std::io::Error::other(error.message))?;
                }
                Ok::<(), std::io::Error>(())
            }
            .await;
            if let Err(error) = result {
                node.log.line(format_args!("resuming delivery for {}: {error}", resident.id));
            }
        });
    }
}

/// What a told answer came to, for the colleague's `messages`.
pub fn outcome(landed: &Result<Landed, Failure>) -> Outcome {
    match landed {
        Ok(Landed::NewTurn { .. } | Landed::JoinedTurn { .. } | Landed::Handed { .. }) => Outcome::Delivered,
        Ok(Landed::Waiting { why, .. }) => Outcome::Waiting { why: why.clone() },
        Ok(Landed::Sent) => Outcome::Sent,
        Ok(Landed::Uncertain { why }) => Outcome::Failed { reason: why.clone() },
        Err(failure) => Outcome::Failed { reason: failure.message.clone() },
    }
}

/// A new notice id: the time and eight random hex digits.
pub fn new_id(prefix: &str) -> NoticeId {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("notice identity needs system randomness");
    let at = Timestamp::now().as_millisecond();
    NoticeId::try_from(format!("{prefix}-{at}-{serial:x}-{:016x}", u64::from_be_bytes(bytes)))
        .expect("a word, digits and hex")
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
