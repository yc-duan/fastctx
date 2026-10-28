//! The user speaking to a resident anywhere in the World and watching it
//! (`world-colleagues.md`, 用户在任何地方和居民说话).
//!
//! The user's words go in as the user's: the machine they were typed on
//! signs the call that carries them, and the session reads them as said by
//! the user on that machine. Nobody is owed the turn they go into; the user
//! watches it. Watching hands over what happened before, as far as the
//! session's host keeps it, then each event as it happens. A resident on
//! another machine is watched over a data connection from its machine, each
//! piece signed there, bound to this watch and numbered, so the hub in
//! between can neither make one up, drop one, nor play an old one again.

use std::sync::Arc;

use fastctx_net::data::Stream;
use fastctx_net::link::Link;
use fastctx_net::sign::Domain;
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::agents::Choices;
use fastctx_types::attachments::Attachment;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::ConversationDir;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{About, Activity, Host, Landed, Now, Origin};
use fastctx_types::session::held::Held;
use fastctx_types::session::{SessionEvent, Watched};
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use fastctx_net::watch_queue::{self, Events};
pub(crate) mod multiplex;

use crate::Node;
use crate::speech::{self, Owed};

/// One piece of a watch from another machine, as that machine signs it.
#[derive(Serialize, Deserialize)]
struct Piece {
    watch: u64,
    n: u64,
    watched: Watched,
}

/// The user's words, typed on this machine, to a resident on any machine.
pub async fn talk(
    node: &Arc<Node>,
    to: ResidentId,
    machine: Option<Written>,
    text: String,
    attachments: Vec<Attachment>,
    files_on: Option<Written>,
    id: Option<String>,
) -> Result<(Address, Landed), Failure> {
    let here = node.machine();
    let local = machine.is_none() && node.residents.about(&to).is_some();
    let machine = match machine {
        Some(machine) => machine,
        None => locate(node, &to).await?,
    };
    if local || node.is_here(&machine) {
        let landed = talk_here(node, &here, &to, text, attachments, files_on.as_ref(), id.as_deref()).await?;
        return Ok((Address { machine: here, resident: to }, landed));
    }
    let call = RemoteCall::Talk { to: to.clone(), text, attachments, files_on, id };
    match node.call(&Acting::Here, &machine, Purpose::Agents, call).await.map_err(crate::authorization::call_failure)? {
        Ok(RemoteReply::Said { landed }) => Ok((Address { machine: machine.name, resident: to }, landed)),
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than a talk"))),
        Err(failure) => Err(failure),
    }
}

/// The user's words, typed on `typed_on`, to a resident of this machine.
/// Attachments retain their original locations; `files_on` is a legacy hint.
pub async fn talk_here(
    node: &Arc<Node>,
    typed_on: &MachineName,
    to: &ResidentId,
    text: String,
    attachments: Vec<Attachment>,
    files_on: Option<&Written>,
    id: Option<&str>,
) -> Result<Landed, Failure> {
    crate::attachments::resolve(node, to, &attachments).await?;
    let attachments = crate::attachments::place(node, to, attachments, files_on).await?;
    let delivery = crate::attachments::delivery(typed_on, text, attachments, id);
    speech::tell_here(node, to, delivery, Owed::Nobody).await
}

/// Watches a resident on any machine. What it hands over comes on the
/// returned channel until the watch ends, with why when something went
/// wrong; dropping the channel ends the watch. A viewer that says what it
/// holds of the history (`held`) is sent it as segments, slimmed.
pub async fn watch(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
    composer: Option<String>,
    held: Option<Held>,
    now: bool,
    goal: Option<fastctx_types::access::HubAccount>,
) -> Result<watch_queue::Receiver<Result<Watched, Failure>>, Failure> {
    let local = machine.is_none() && node.residents.about(&resident).is_some();
    let machine = match machine {
        Some(machine) => machine,
        None => locate(node, &resident).await?,
    };
    let (sender, receiver) = watch_queue::channel(|piece: &Result<Watched, Failure>| match piece {
        Ok(piece) => watch_queue::watched_size(piece),
        Err(error) => (error.message.len(), None),
    });
    if local || node.is_here(&machine) {
        let mut told = Told::new(node, &resident, now && composer.is_none());
        let (lease, opened) = if let Some(account) = &goal {
            if !node.serves(&Acting::Here, &account.hub, &account.account) {
                return Err(Failure::refused(fastctx_types::goals::words::WATCH_ACCOUNT));
            }
            (None, crate::goals::watch::opening(node, &resident, account).await?)
        } else {
            opening(node, &resident, composer, held.as_ref()).await?
        };
        let Opening { pieces, mut events, slim } = opened;
        let node = node.clone();
        tokio::spawn(async move {
            let _lease = lease;
            for piece in pieces.into_iter().chain(told.first()) {
                if sender.send_wait(Ok(piece), std::time::Duration::from_secs(30)).await.is_err() { return; }
            }
            loop {
                tokio::select! {
                    event = events.recv() => match event {
                        Ok(Some(event)) => {
                            if goal.is_some() && !crate::goals::watch::is_goal(&event) { continue; }
                            if !crate::goals::visible(&node, crate::admission::seen::Looker::Here(&Acting::Here), &event) { continue; }
                            let Some(event) = live(Arc::unwrap_or_clone(event), slim.as_ref()) else { continue };
                            if sender.send(Ok(Watched::Live { event })).is_err() { return }
                        }
                        Ok(None) => return,
                        Err(_) => { sender.lag(); return; },
                    },
                    now = told.changed(&node) => {
                        if let Some(now) = now && sender.send(Ok(now)).is_err() { return }
                    }
                    () = sender.closed() => return,
                }
            }
        });
        return Ok(receiver);
    }
    let acting = goal.as_ref().map(|account| Acting::one(account.hub.clone(), account.account.clone())).unwrap_or(Acting::Here);
    if let Some(account) = &goal && !node.serves(&Acting::Here, &account.hub, &account.account) {
        return Err(Failure::refused(fastctx_types::goals::words::WATCH_ACCOUNT));
    }
    let request = fastctx_types::watching::Subscription { resident, composer, documents: None, held, now, goal: goal.is_some() };
    let mut watching = None;
    let mut refused = None;
    for (link, account) in node.speakers(&acting, &machine, Purpose::Agents).map_err(|e| Failure::new(e.to_string()))? {
        match link.watch_shared(&machine.name, &account, request.clone()).await {
            Ok(opened) => { watching = Some(opened); break; }
            Err(failure) if failure.not_run => refused = Some(failure),
            Err(failure) => return Err(failure),
        }
    }
    let Some(mut watching) = watching else { return Err(refused.expect("a speaker was tried")); };
    tokio::spawn(async move {
        loop {
            let piece = tokio::select! { piece = watching.next() => piece, () = sender.closed() => return };
            match piece {
                Ok(Some(piece)) => {
                    let history = !matches!(piece, Watched::Live { .. } | Watched::Now { .. });
                    let sent = if history { sender.send_wait(Ok(piece), std::time::Duration::from_secs(30)).await }
                        else { sender.send(Ok(piece)) };
                    if sent.is_err() { return; }
                }
                Ok(None) => return,
                Err(failure) => { let _ = sender.send(Err(failure)); return; }
            }
        }
    });
    Ok(receiver)
}

/// What opening a watch of a resident of this machine hands over first,
/// the events after it, and whose they are when they go slimmed.
pub(crate) struct Opening {
    pub pieces: Vec<Watched>,
    pub events: Events,
    /// The session, when what follows goes slimmed, its images referred to
    /// where it keeps them.
    pub slim: Option<Address>,
}

/// Opens a watch of a resident of this machine: of its shared composer,
/// with no history; of its session for a viewer that holds part of the
/// history, as segments; otherwise its whole history.
async fn opening(
    node: &Arc<Node>,
    resident: &ResidentId,
    composer: Option<String>,
    held: Option<&Held>,
) -> Result<(Option<crate::composer::Lease>, Opening), Failure> {
    if let Some(name) = composer {
        let lease = crate::composer::watch(node, resident, name).await?;
        let events = node.watchers.watch_composer(resident);
        crate::composer::snapshot(node, resident).await?;
        let pieces = vec![Watched::History { events: Vec::new() }];
        return Ok((Some(lease), Opening { pieces, events, slim: None }));
    }
    if let Some(held) = held {
        return Ok((None, crate::held_history::opening(node, resident, held).await?));
    }
    let (history, events) = watch_here(node, resident).await?;
    Ok((None, Opening { pieces: vec![Watched::History { events: history }], events, slim: None }))
}

/// A live event as a watcher is sent it: slimmed for one that holds the
/// history; `None` when nothing of it is drawn.
pub(crate) fn live(mut event: SessionEvent, slim: Option<&Address>) -> Option<SessionEvent> {
    match slim {
        Some(owner) => fastctx_net::history::slim(&mut event, &fastctx_net::history::Pictures::Referenced { owner })
            .then_some(event),
        None => Some(event),
    }
}

/// A resident of this machine: its history, and the events from now on.
/// The watch starts before the history is read, so nothing falls between;
/// what is in both is the watcher's to recognise.
pub async fn watch_here(
    node: &Arc<Node>,
    resident: &ResidentId,
) -> Result<(Vec<SessionEvent>, Events), Failure> {
    let read = history_here(node, resident).await?;
    let mut history = read.history;
    history.extend(read.pending);
    Ok((history, read.events))
}

/// A resident's history as read now, apart from what it waits on.
pub(crate) struct HistoryRead {
    pub history: Vec<SessionEvent>,
    /// The requests its host waits on someone to answer now: not part of
    /// the history, and gone once answered.
    pub pending: Vec<SessionEvent>,
    pub events: Events,
}

/// Starts watching a resident of this machine, without reading its history:
/// the events from now on, and the requests its host waits on someone to
/// answer now.
pub(crate) fn subscribe(
    node: &Arc<Node>,
    resident: &ResidentId,
) -> Result<(Events, Vec<SessionEvent>), Failure> {
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    let events = node.watchers.watch(resident);
    match &about.session {
        Some(thread) if about.host.agent.as_str() == "codex" => {
            let codex = node.codex.of(node, &about)?;
            let pending = if about.world_managed() { codex.requests(thread) } else { Vec::new() };
            Ok((crate::codex::watch::requests(codex, thread.clone(), events), pending))
        }
        _ => Ok((events, Vec::new())),
    }
}

/// Starts watching a resident of this machine, then reads its history: what
/// the World kept of it, merged with what its host keeps.
pub(crate) async fn history_here(node: &Arc<Node>, resident: &ResidentId) -> Result<HistoryRead, Failure> {
    read_here(node, resident).await.map_err(|failure| as_deleted(node, resident, failure))
}

/// A read that failed as the conversation was being deleted, or once it is
/// gone, says so, not what its host or its files said as they went.
fn as_deleted(node: &Node, resident: &ResidentId, failure: Failure) -> Failure {
    if node.residents.is_deleting(resident) {
        Failure::new(format!("{resident} is being deleted"))
    } else if node.residents.about(resident).is_none() {
        crate::forget::gone(node, resident)
    } else {
        failure
    }
}

async fn read_here(node: &Arc<Node>, resident: &ResidentId) -> Result<HistoryRead, Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
    // A session the user opened on their own is theirs to read, in their
    // own program (`world-layers.md`).
    if !about.world_managed() {
        return Err(Failure::refused(crate::speech::OWN_SESSION));
    }
    let mut events = node.watchers.watch(resident);
    let directory = node.world.conversation(resident);
    let recorded = |directory: &ConversationDir| {
        crate::delivery::history(directory).map_err(|error| Failure::new(error.to_string()))
    };
    let owner = Address { machine: node.machine(), resident: resident.clone() };
    let (recorded, hosted) = match &about.session {
        // Codex keeps every thread of its home, whoever opened it: the World
        // reads the history of one the user opened in the Codex command line
        // from the same record as one it started.
        Some(thread) if about.host.agent.as_str() == "codex" => {
            crate::homes::available(node, &about)?;
            let codex = node.codex.of(node, &about)?;
            let history = codex.history(thread, &owner).await.map_err(Failure::new)?;
            codex.flush_events(thread).await.map_err(Failure::new)?;
            // Reverted turns remain in the World's audit record, but no
            // longer belong to the host's current conversation.
            let recorded = crate::codex::watch::recorded(recorded(&directory)?, &history);
            // A request can arrive and resolve while history is loading.
            // Only currently unanswered requests may enter a new view; the
            // subscription already covers changes after this snapshot.
            events = crate::codex::watch::requests(codex, thread.clone(), events);
            (recorded, Some(history))
        }
        // Claude Code's own record of the session, placed in the turns the
        // World saw, which the World's record says.
        _ if about.host.agent.as_str() == "claude" => {
            let mut recorded = recorded(&directory)?;
            let removed: std::collections::HashSet<_> = recorded
                .iter()
                .flat_map(|event| match &event.happened {
                    fastctx_types::session::Happened::Changed {
                        change: fastctx_types::session::SessionChange::HistoryCut { removed_turns },
                        ..
                    } => removed_turns.clone(),
                    _ => Vec::new(),
                })
                .collect();
            recorded.retain(|event| turn_of(&event.happened).is_none_or(|turn| !removed.contains(turn)));
            let hosted = crate::claude::history::hosted(node, resident, &recorded).await;
            (recorded, Some(hosted))
        }
        _ => (recorded(&directory)?, None),
    };
    let receipts = crate::attachments::delivered::of(node, resident);
    let mut history = history(recorded, hosted, about.host.agent.as_str(), &receipts);
    // A change of history tells a view already open to read again; the
    // history read here is already what follows it. Kept in a view, an
    // interface that reads it as news would read again on every event.
    history.retain(|event| {
        !matches!(
            &event.happened,
            fastctx_types::session::Happened::Changed {
                change: fastctx_types::session::SessionChange::HistoryChanged
                    | fastctx_types::session::SessionChange::HistoryCut { .. },
                ..
            }
        )
    });
    for event in &mut history {
        crate::run_facts::attach(&directory, event).map_err(|error| Failure::new(error.to_string()))?;
    }
    let pending = match &about.session {
        Some(thread) if about.world_managed() && about.host.agent.as_str() == "codex" => {
            node.codex.of(node, &about)?.requests(thread)
        }
        _ => Vec::new(),
    };
    Ok(HistoryRead { history, pending, events })
}

/// A session's history: what the World kept of it, and what its host keeps
/// that the World does not (`crate::turns::recorded`). Where both tell a
/// turn's start or end, the World's account stands: it knows why a turn was
/// cut short. Words the World sent are told by its own record, which knows
/// who said them; the host's copy of them carries the World's head note or,
/// in Codex, the id the World gave the message.
fn history(
    recorded: Vec<SessionEvent>,
    hosted: Option<Vec<SessionEvent>>,
    agent: &str,
    receipts: &dyn fastctx_types::session::attachments::Receipts,
) -> Vec<SessionEvent> {
    use fastctx_types::session::Happened;
    let Some(hosted) = hosted else {
        return recorded_history(recorded);
    };
    let sent_turns: std::collections::HashSet<_> = recorded
        .iter()
        .filter_map(|event| match &event.happened {
            Happened::Heard { turn: Some(turn), .. } => Some(turn.as_str()),
            _ => None,
        })
        .collect();
    let mut history: Vec<SessionEvent> = hosted
        .into_iter()
        .flat_map(|event| fastctx_types::session::attachments::restore(event, receipts))
        .filter(|event| match &event.happened {
            Happened::Heard { message: Some(message), .. } if !message.images.is_empty() => true,
            Happened::Heard { delivery: Some(_), message: Some(message), .. } if !message.attachments.is_empty() => {
                true
            }
            // Other words the World told are its own record's, except, for
            // Codex, in a turn the World has no record of sending into. The
            // World's head note tells them, and in Codex, where the user's
            // words go in as typed, the id the World gave the message.
            Happened::Heard { turn, text, message, .. } => {
                let told = fastctx_hands::told::is_told(text)
                    || agent == "codex" && message.as_deref().is_some_and(fastctx_hosts::codex::sent_by_world);
                !(told && (agent != "codex" || turn.as_deref().is_some_and(|turn| sent_turns.contains(turn))))
            }
            _ => true,
        })
        .collect();
    // What the host's record holds is the one shown of anything both tell
    // under one name: a Claude reply seen as it streamed is recorded under
    // the name its row in Claude Code's file gives it.
    let held: std::collections::HashSet<(String, String)> = history
        .iter()
        .filter_map(|event| match &event.happened {
            Happened::Finished { turn, item } | Happened::Updated { turn, item } => {
                Some((turn.clone(), item.id.clone()))
            }
            _ => None,
        })
        .collect();
    // A draft recorded before its row was kept (a reply cut short, kept
    // after its stream ended) is the item that names it as its draft.
    let drafts: std::collections::HashSet<(String, String)> = recorded
        .iter()
        .filter_map(|event| match &event.happened {
            Happened::Finished { turn, item } => Some((turn.clone(), item.draft.clone()?)),
            _ => None,
        })
        .collect();
    for mut event in recorded {
        if let Happened::Finished { turn, item } | Happened::Updated { turn, item } = &event.happened
            && (held.contains(&(turn.clone(), item.id.clone())) || drafts.contains(&(turn.clone(), item.id.clone())))
        {
            continue;
        }
        // Native words are reconstructed from the host, including the
        // stable picture coordinates. Older nodes also recorded them here.
        if matches!(&event.happened, Happened::Heard { delivery: None, from: None, message: Some(message), .. }
            if agent == "codex" && message.detail["type"] == "userMessage")
        {
            continue;
        }
        let same = history.iter().position(|other| match (&event.happened, &other.happened) {
            (Happened::TurnStarted { turn: a }, Happened::TurnStarted { turn: b })
            | (Happened::TurnEnded { turn: a, .. }, Happened::TurnEnded { turn: b, .. }) => a == b,
            (
                Happened::Heard { turn: a, message: Some(a_message), .. },
                Happened::Heard { turn: b, message: Some(b_message), .. },
            ) => a == b && a_message.id == b_message.id,
            (Happened::Realtime { item: a, complete: ac }, Happened::Realtime { item: b, complete: bc }) => {
                ac == bc && a["id"] == b["id"] && a["realtimeSessionId"] == b["realtimeSessionId"]
            }
            _ => false,
        });
        if let Some(index) = same {
            crate::user_images::merge(&mut event, &history[index]);
            history[index] = event;
            continue;
        }
        let turn = turn_of(&event.happened);
        let index = history
            .iter()
            .position(|other| {
                let same_turn = turn.is_some() && turn == turn_of(&other.happened);
                if same_turn && matches!(other.happened, Happened::TurnEnded { .. }) {
                    return true;
                }
                if same_turn && matches!(event.happened, Happened::TurnStarted { .. }) {
                    return true;
                }
                if matches!(other.happened, Happened::TurnStarted { .. }) && same_turn {
                    return false;
                }
                other.at > event.at
            })
            .unwrap_or(history.len());
        history.insert(index, event);
    }
    history.retain(|event| event.happened.in_history());
    // Preserve the host's canonical timeline, including interleaved voice
    // items and steering input. Local facts were inserted at their source time.
    history
}

fn recorded_history(mut history: Vec<SessionEvent>) -> Vec<SessionEvent> {
    use fastctx_types::session::Happened;
    history.retain(|event| event.happened.kept());
    // The host's record keeps times in whole seconds, the World's to the
    // microsecond, so time alone would put an answer before the words that
    // asked for it. Each turn stands where it began, and within it come its
    // start, the words it heard, what was done in the order the record holds
    // it, and its end. What belongs to no turn stands at its own time.
    let mut began: std::collections::HashMap<String, jiff::Timestamp> = std::collections::HashMap::new();
    for event in &history {
        if let Some(turn) = turn_of(&event.happened) {
            began.entry(turn.to_string()).and_modify(|at| *at = (*at).min(event.at)).or_insert(event.at);
        }
    }
    history.sort_by_key(|event| {
        let at = match &event.happened {
            Happened::Finished { item, .. } => item.started_at.unwrap_or(event.at),
            _ => event.at,
        };
        let phase = match &event.happened {
            Happened::TurnStarted { .. } => 0,
            Happened::Heard { .. } => 1,
            Happened::TurnEnded { .. } => 3,
            _ => 2,
        };
        match turn_of(&event.happened).and_then(|turn| began.get(turn)) {
            Some(start) => (*start, phase, at),
            None => (at, 2, at),
        }
    });
    history
}

/// The turn an event belongs to, where it says.
pub(crate) fn turn_of(happened: &fastctx_types::session::Happened) -> Option<&str> {
    use fastctx_types::session::Happened;
    match happened {
        Happened::TurnStarted { turn }
        | Happened::TurnEnded { turn, .. }
        | Happened::Began { turn, .. }
        | Happened::Updated { turn, .. }
        | Happened::Grew { turn, .. }
        | Happened::Finished { turn, .. }
        | Happened::Steps { turn, .. } => (!turn.is_empty()).then_some(turn),
        Happened::Heard { turn, .. } | Happened::Health { turn, .. } | Happened::Changed { turn, .. } => {
            turn.as_deref()
        }
        Happened::Context { .. }
        | Happened::Titled { .. }
        | Happened::DeliveryUncertain { .. }
        | Happened::Realtime { .. }
        | Happened::RealtimeGrew { .. } => None,
    }
}


/// Hands a watch of a resident of this machine to whoever asked through
/// `link`, over `stream`, until they close it; each piece is signed here,
/// packed for a viewer that holds the history.
#[allow(clippy::too_many_arguments)]
pub async fn serve_watch(
    node: &Arc<Node>,
    asker: &crate::admission::Asker,
    link: &Arc<Link>,
    from: &MachineName,
    resident: ResidentId,
    watch: u64,
    composer: Option<String>,
    held: Option<Held>,
    now: bool,
    goal: bool,
    mut stream: Stream,
) -> Result<RemoteReply, Failure> {
    let what = match &composer {
        Some(_) => format!("{from}'s watch of {resident}'s shared input"),
        None => format!("{from}'s watch of {resident}"),
    };
    let started = std::time::Instant::now();
    let ended = |why: &str| -> Result<RemoteReply, Failure> {
        node.log.line(format_args!("{what} ended after {}: {why}", lasted(started)));
        Ok(RemoteReply::Watched)
    };
    let packed = held.is_some();
    let mut told = Told::new(node, &resident, now && composer.is_none());
    let open = async {
        if goal {
            let account = fastctx_types::access::HubAccount { hub: asker.hub.clone(), account: asker.account.clone() };
            Ok((None, crate::goals::watch::opening(node, &resident, &account).await?))
        } else {
            opening(node, &resident, composer, held.as_ref()).await
        }
    };
    let (_lease, opened) = tokio::select! {
        result = open => match result {
            Ok(opened) => opened,
            Err(failure) => {
                node.log.line(format_args!("{what} could not open: {}", failure.message));
                // Closed rather than dropped, so the watcher waits for the
                // answer that says why.
                stream.close().await;
                return Err(failure);
            }
        },
        why = stream.ended() => return ended(&format!("it left before it opened ({})", why.as_deref().unwrap_or("closed"))),
    };
    node.log.line(format_args!("{what} opened"));
    let Opening { pieces, mut events, slim } = opened;
    let mut n = 0;
    let mut next: std::collections::VecDeque<Watched> = pieces.into();
    next.extend(told.first());
    loop {
        while let Some(watched) = next.pop_front() {
            let piece = Piece { watch, n, watched };
            let sent = if packed {
                // A history's segments take a while to pack (0.4 s for a
                // long Claude session's 50 MB); they are packed off the
                // runtime's threads, which serve everything else here.
                let bytes = match piece.watched {
                    Watched::Live { .. } => fastctx_net::history::pack(&piece),
                    _ => tokio::task::spawn_blocking(move || fastctx_net::history::pack(&piece))
                        .await
                        .map_err(|error| Failure::new(format!("packing the history: {error}")))?,
                };
                let signed = link.sign_bytes(Domain::Event, bytes);
                fastctx_net::watch_stream::send_bytes(&mut stream, &signed).await
            } else {
                fastctx_net::watch_stream::send(&mut stream, &link.sign(Domain::Event, &piece)).await
            };
            if let Err(why) = sent {
                return ended(&format!("sending: {why}"));
            }
            n += 1;
        }
        tokio::select! {
            event = events.recv() => match event {
                Ok(Some(event)) => {
                    if (!goal || crate::goals::watch::is_goal(&event)) && crate::goals::visible(node, crate::admission::seen::Looker::Asker(asker), &event) {
                        next.extend(live(Arc::unwrap_or_clone(event), slim.as_ref()).map(|event| Watched::Live { event }));
                    }
                },
                Ok(None) => return ended("this machine stopped telling its events"),
                Err(error) => { stream.close().await; return Err(Failure::new(error.to_string())); },
            },
            now = told.changed(node) => next.extend(now),
            why = stream.ended() => return ended(why.as_deref().unwrap_or("it closed the watch")),
        }
    }
}

/// What a watch tells of whether its resident works now
/// (`Watched::Now`), for a viewer that asked: once the history is told, then
/// on each change. The residents' change count wakes it for any of them; it
/// tells only when this one's changed.
struct Told {
    resident: ResidentId,
    changes: Option<tokio::sync::watch::Receiver<u64>>,
    last: Option<(bool, Option<String>, Vec<fastctx_types::session::KeptWords>)>,
}

impl Told {
    fn new(node: &Node, resident: &ResidentId, asked: bool) -> Self {
        Self { resident: resident.clone(), changes: asked.then(|| node.residents.changes()), last: None }
    }

    /// The resident's state as its first telling, when asked for.
    fn first(&mut self) -> Option<Watched> {
        self.changes.as_ref()?;
        self.last = None;
        None
    }

    /// The next telling: waits for a change of any resident, and is `Some`
    /// once this one's state is not what was told last. Never ready when
    /// the viewer did not ask.
    async fn changed(&mut self, node: &Node) -> Option<Watched> {
        let Some(changes) = &mut self.changes else { return std::future::pending().await };
        if self.last.is_some() {
            // A change of words kept moves no resident: while some are kept
            // or it works, it is looked at again every few seconds too.
            let unsettled = self.last.as_ref().is_some_and(|(busy, _, kept)| *busy || !kept.is_empty());
            let again =
                async { if unsettled { tokio::time::sleep(LOOK_AGAIN).await } else { std::future::pending().await } };
            tokio::select! {
                changed = changes.changed() => if changed.is_err() { return std::future::pending().await },
                () = again => {}
            }
        }
        let (busy, turn) = node.residents.now(&self.resident).map_or((false, None), |now| {
            (now.activity == fastctx_types::residents::Activity::Busy || now.turn.is_some(), now.turn)
        });
        let kept = kept(node, &self.resident);
        let state = (busy, turn, kept);
        if self.last.as_ref() == Some(&state) {
            return None;
        }
        self.last = Some(state.clone());
        let (busy, turn, kept) = state;
        Some(Watched::Now { busy, turn, kept })
    }
}

/// While a resident works or words are kept for it, how often a watch that
/// tells it looks again.
const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_secs(5);

/// The words kept for `resident` here that no turn has taken yet, and how far
/// each came.
fn kept(node: &Node, resident: &ResidentId) -> Vec<fastctx_types::session::KeptWords> {
    use fastctx_types::residents::PendingState;
    use fastctx_types::session::{KeptWords, WordsState};
    let directory = node.world.conversation(resident);
    let managed = node.residents.about(resident).is_some_and(|(about, _)| about.world_started);
    let Ok(pending) = crate::inbox::pending(&directory) else { return Vec::new() };
    // The words that would start its next turn, while that waits for room
    // in its login.
    let mut queued = crate::turn_slots::ahead(node, resident);
    pending
        .into_iter()
        .map(|pending| {
            let first = pending.state == PendingState::Waiting && pending.delivery.notice.along().is_none();
            let progress = crate::lifecycle::retry::progress(node, resident, &pending.delivery.id).ok().flatten();
            let retrying = progress.is_some();
            let host_retry = progress
                .filter(|progress| Timestamp::now().as_second().saturating_sub(progress.began.as_second()) >= 60)
                .and_then(|progress| {
                    progress.reason.map(|reason| fastctx_types::session::HostRetry { since: progress.began, reason })
                });
            KeptWords {
                id: pending.delivery.id,
                state: match pending.state {
                    PendingState::Waiting if !retrying && !managed => WordsState::Waiting,
                    _ => WordsState::Handing,
                },
                host_retry,
                queued: if first { queued.take() } else { None },
            }
        })
        .collect()
}

/// How long something lasted, as a log line says it.
pub(crate) fn lasted(since: std::time::Instant) -> String {
    let seconds = since.elapsed().as_secs();
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{}m{:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// Opens a session for the user on this machine or on `target`, working
/// for the account the user chose, or for this computer's logins where it
/// lives here and they chose none.
#[allow(clippy::too_many_arguments)]
pub async fn open(
    node: &Arc<Node>,
    agent: AgentName,
    town: Option<TownName>,
    target: Option<Written>,
    text: String,
    options: fastctx_types::agents::SessionOptions,
    attachments: Vec<Attachment>,
    choices: Choices,
) -> Result<(Address, Landed), Failure> {
    let here = node.machine();
    let acting = match &choices.account {
        Some(account) => {
            if !node.serves(&Acting::Here, &account.hub, &account.account) {
                return Err(Failure::not_run(format!(
                    "this computer is not logged in to the account {}",
                    account.account
                )));
            }
            Acting::one(account.hub.clone(), account.account.clone())
        }
        None => {
            // A conversation opened from the World's interface works for one
            // account; with one logged in here, that one.
            let logins: Vec<_> = node
                .links()
                .iter()
                .flat_map(|link| {
                    let hub = link.membership().id.clone();
                    let logins = link.view().me().map(|me| me.logins.clone()).unwrap_or_default();
                    logins
                        .into_iter()
                        .map(move |account| fastctx_types::access::HubAccount { hub: hub.clone(), account })
                })
                .collect();
            match logins.as_slice() {
                [] => Acting::Here,
                [only] => Acting::one(only.hub.clone(), only.account.clone()),
                _ => {
                    return Err(Failure::not_run(
                        "this computer is logged in to more than one account; choose the one the conversation works for",
                    ));
                }
            }
        }
    };
    match target.filter(|target| !node.is_here(target)) {
        None => {
            let (id, landed) = open_here(node, &here, acting, agent, town, text, options, attachments, choices).await?;
            Ok((Address { machine: here, resident: id }, landed))
        }
        Some(target) => {
            let profile = crate::profile::session::get(node, &acting);
            if !choices.mcp_off.is_empty()
                || !profile.prompt.is_empty()
                || !profile.claude.is_empty()
                || !profile.codex.is_empty()
                || !profile.servers.is_empty()
            {
                let (link, account) =
                    node.speaking(&acting, &target, Purpose::Agents).map_err(|e| Failure::new(e.to_string()))?;
                crate::mcp::supports(node, &link, &account, &target.name).await?;
            }
            let call = RemoteCall::Open {
                mcp_off: choices.mcp_off.clone(),
                agent,
                town,
                text,
                model: options.model,
                reasoning_effort: options.reasoning_effort,
                attachments,
                focus: choices.focus,
                home: choices.home,
                files_on: choices.files_on,
            };
            let reply = node.call(&acting, &target, Purpose::Agents, call).await;
            match reply.map_err(|failure| Failure::new(failure.to_string()))? {
                Ok(RemoteReply::Opened { resident, landed }) => {
                    Ok((Address { machine: target.name, resident }, landed))
                }
                Ok(_) => Err(Failure::new(format!("{target} answered something other than an opened session"))),
                // What went wrong there is said of that machine, not this one.
                Err(failure) => Err(Failure::new(format!("on {target}: {}", failure.message))),
            }
        }
    }
}

/// Opens a session on this machine for the user typing on `typed_on`, with
/// their words as its first, working for `acting`.
#[allow(clippy::too_many_arguments)]
pub async fn open_here(
    node: &Arc<Node>,
    typed_on: &MachineName,
    acting: Acting,
    agent: AgentName,
    town: Option<TownName>,
    text: String,
    options: fastctx_types::agents::SessionOptions,
    attachments: Vec<Attachment>,
    choices: Choices,
) -> Result<(ResidentId, Landed), Failure> {
    // The login it is born in must be in this machine's agent list, the
    // default one too (`world-agent-logins.md`); the default one is recorded
    // as none, so that it follows the user's environment.
    crate::layers::may_start(node, &acting)?;
    let mut options = options;
    options.mcp_off = choices.mcp_off;
    let home = crate::homes::listed(node, agent.as_str(), choices.home.as_deref())?;
    let opened = Opened { acting, focus: choices.focus, home, files_on: choices.files_on };
    match agent.as_str() {
        "codex" => {}
        "claude" => return crate::claude::open_here(node, typed_on, opened, town, text, options, attachments).await,
        _ => {
            return Err(Failure::new(format!(
                "the World opens codex and claude sessions; it cannot open {agent} ones"
            )));
        }
    }
    if let Some(town) = &town {
        crate::town::access::ready(node, &opened.acting, town).await?;
    }
    let (id, _opening) = crate::attachments::opening(node, &agent, &opened.acting, town.as_ref(), &attachments).await?;
    let origin = Origin::User;
    let _birth = crate::here::born(node, &id, opened.acting.clone(), &origin);
    crate::session_options::keep(node, &id, &options)?;
    // What it is told of its machines and folders goes into the instructions
    // its thread starts with.
    crate::focus::open(node, &node.world.conversation(&id), &opened.acting, &opened.focus).await?;
    let (spec, heard) =
        crate::colleagues::thread_spec(node, &id, &origin, town.as_ref(), &opened.acting, opened.home.as_deref())
            .await?;
    // Until it is claimed below, reports of threads nobody claims are kept.
    let _starting = crate::codex::starting(node);
    let (thread, model) = node
        .codex
        .session(opened.home.as_deref(), crate::assists::host::kept(node, &id)?.unwrap_or(false))?
        .start_thread(spec)
        .await
        .map_err(Failure::new)?;
    let born_at = Timestamp::now();
    let about = About {
        side: false,
        forked_from: None,
        mcp_off: options.mcp_off.clone(),
        id: id.clone(),
        host: Host { agent, client: "codex app-server".to_string(), version: String::new() },
        machine: node.machine(),
        born_at,
        origin,
        session: Some(thread.clone()),
        world_started: true,
            native_resumed: false,
        model,
        acting: opened.acting,
        focus: opened.focus,
        home: opened.home,
        callee: None,
    };
    let now = Now {
        activity: Activity::Idle,
        since: born_at,
        turn: None,
        last_turn: None,
        owed: Default::default(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
        desk_since: None,
        waiting_for_user: false,
        waiting_since: None,
        title: None,
        last_tool_at: None,
        first_words: None,
        last_said: None,
        last_compacted: false,
        last_heard_user: None,
        asked: None,
        told: None,
    };
    if let Some(town) = &town {
        crate::town::access::check(node, &about.acting, town)?;
    }
    node.residents
        .create(&node.world, about, town, now)
        .map_err(|error| Failure::new(format!("recording the session: {error}")))?;
    crate::context::instructed(node, &id, &heard).map_err(|error| Failure::new(error.to_string()))?;
    node.residents.set_instructed(&id, heard);
    crate::codex::claim(node, &thread, &id).await;
    let landed = talk_here(node, typed_on, &id, text, attachments, opened.files_on.as_ref(), None).await?;
    Ok((id, landed))
}

/// Stops the turn a resident on any machine is running; returns where it
/// lives and whether a turn was running.
pub async fn stop(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
) -> Result<(Address, bool), Failure> {
    let here = node.machine();
    let local = machine.is_none() && node.residents.about(&resident).is_some();
    let machine = match machine {
        Some(machine) => machine,
        None => locate(node, &resident).await?,
    };
    if local || node.is_here(&machine) {
        let stopped = stop_here(node, &resident).await?;
        return Ok((Address { machine: here, resident }, stopped));
    }
    let call = RemoteCall::Stop { resident: resident.clone() };
    let reply = node.call(&Acting::Here, &machine, Purpose::Agents, call).await;
    match reply.map_err(|failure| Failure::new(failure.to_string()))? {
        Ok(RemoteReply::Stopped { stopped }) => Ok((Address { machine: machine.name, resident }, stopped)),
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than a stop"))),
        Err(failure) => Err(failure),
    }
}

/// Stops the turn a resident of this machine is running: a Codex thread of
/// this node's app-server, through the app-server; a Claude session, any
/// whose plugin is paired with the World, through its mod. A Codex session
/// of the user's own belongs to the program it is open in.
pub async fn stop_here(node: &Arc<Node>, resident: &ResidentId) -> Result<bool, Failure> {
    stop_here_as(node, resident, fastctx_types::turn_evidence::StopSource::User).await
}

pub(crate) async fn stop_here_as(
    node: &Arc<Node>,
    resident: &ResidentId,
    source: fastctx_types::turn_evidence::StopSource,
) -> Result<bool, Failure> {
    node.residents.stoppable(resident)?;
    let targeted = node.residents.now(resident).and_then(|now| now.turn);
    if source == fastctx_types::turn_evidence::StopSource::User {
        crate::goals::holding::stop(node, resident, targeted.clone())?;
    }
    if let Some(turn) = &targeted {
        crate::turn_evidence::remember(node, resident, turn, source)
            .map_err(|error| Failure::new(format!("recording why the turn is stopped: {error}")))?;
        if source == fastctx_types::turn_evidence::StopSource::User {
            crate::stops::mark_turn(node, resident, turn).map_err(|error| Failure::new(error.to_string()))?;
        }
    }
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    // What its run calls wait for stops at once, here and on the machines it
    // works on, not once its host has ended the turn.
    node.interrupts.raise(&Address { machine: node.machine(), resident: resident.clone() });
    // Words on their way start a turn whose id is not known yet: it stops as
    // soon as it is. What only rides with the next turn starts none, and
    // keeps no stop for the turn someone starts later.
    let directory = node.world.conversation(resident);
    let coming = tokio::task::spawn_blocking(move || crate::inbox::pending(&directory))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("reading what is on its way: {error}")))?;
    let coming = coming
        .into_iter()
        .filter(|pending| pending.delivery.notice.along().is_none())
        .map(|pending| pending.delivery.id)
        .collect();
    if targeted.is_none() && node.stops.coming(node, resident, coming, source).map_err(|error| Failure::new(error.to_string()))? {
        return Ok(true);
    }
    let current = node.residents.now(resident).and_then(|now| now.turn);
    if targeted.is_some() && targeted != current {
        return Ok(false);
    }
    let targeted = targeted.or(current);
    if let Some(turn) = &targeted {
        crate::turn_evidence::remember(node, resident, turn, source)
            .map_err(|error| Failure::new(format!("recording why the turn is stopped: {error}")))?;
        if source == fastctx_types::turn_evidence::StopSource::User {
            crate::stops::mark_turn(node, resident, turn).map_err(|error| Failure::new(error.to_string()))?;
        }
    }
    match (about.host.agent.as_str(), about.world_managed(), &about.session) {
        ("codex", true, Some(thread)) => match targeted {
            Some(turn) => node.codex.of(node, &about)?.interrupt_turn(thread, &turn).await.map_err(Failure::new),
            None => node.codex.of(node, &about)?.interrupt(thread).await.map_err(Failure::new),
        },
        ("claude", _, _) => match targeted {
            Some(turn) => crate::claude::stop(node, resident, &turn),
            None => Ok(false),
        },
        _ => Err(Failure::new(format!("{resident} is a session the World does not hold; stop it where it is open"))),
    }
}

/// Stops the turn the resident `id` of this machine runs, if it runs one,
/// and waits for that turn to end as turns end, from its host.
pub async fn stop_and_wait(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let mut changes = node.residents.changes();
    let running = |node: &Node| node.residents.now(id).is_some_and(|now| now.turn.is_some());
    if !running(node) {
        return Ok(());
    }
    stop_here_as(node, id, fastctx_types::turn_evidence::StopSource::Lifecycle).await?;
    while running(node) {
        changes.changed().await.map_err(|_| Failure::new("the node is stopping"))?;
    }
    Ok(())
}

/// Which machine the resident `id` lives on: this one when it lives here,
/// otherwise whichever says it does.
async fn locate(node: &Arc<Node>, id: &ResidentId) -> Result<Written, Failure> {
    if node.residents.about(id).is_some() {
        return Ok(node.machine().into());
    }
    if !node.connected_anywhere() {
        return Err(crate::forget::gone(node, id));
    }
    crate::colleagues::find_for(node, &Acting::Here, id).await
}

/// What the user chose for a session opened here, as this machine keeps it.
pub struct Opened {
    pub acting: Acting,
    /// The machines and folders the user points it to.
    pub focus: fastctx_types::focus::Focus,
    /// The agent's home here it is born in, when not the default one.
    pub home: Option<std::path::PathBuf>,
    /// Where the files attached to its first words go, other than images,
    /// when the user chose.
    pub files_on: Option<Written>,
}
