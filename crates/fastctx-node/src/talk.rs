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

use crate::watch::Events;
use fastctx_net::data::Stream;
use fastctx_net::link::Link;
use fastctx_net::sign::Domain;
use fastctx_net::watch_queue;
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
mod forwarding;
mod likeness;
pub(crate) mod merging;
pub(crate) mod multiplex;
pub(crate) mod submission;

use crate::Node;
use crate::speech::{self, Owed};

/// One piece of a watch from another machine, as that machine signs it.
#[derive(Serialize, Deserialize)]
pub(crate) struct Piece {
    pub(crate) watch: u64,
    pub(crate) n: u64,
    pub(crate) watched: Watched,
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
        let landed = talk_here(node, None, &here, &to, text, attachments, files_on.as_ref(), id.as_deref()).await?;
        return Ok((Address { machine: here, resident: to }, landed));
    }
    let call = RemoteCall::Talk { to: to.clone(), text, attachments, files_on, id };
    let asking = crate::submitted::Asking::begin();
    let reply = loop {
        let reply = node
            .call(&Acting::Here, &machine, Purpose::Agents, call.clone())
            .await
            .map_err(crate::authorization::call_failure)?;
        if let Some(reply) = asking.settled(reply).await {
            break reply;
        }
    };
    match reply {
        Ok(RemoteReply::Said { landed }) => Ok((Address { machine: machine.name, resident: to }, landed)),
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than a talk"))),
        Err(failure) => Err(failure),
    }
}

/// The user's words, typed on `typed_on`, to a resident of this machine.
/// Attachments retain their original locations; `files_on` is a legacy hint.
#[allow(clippy::too_many_arguments)]
pub async fn talk_here(
    node: &Arc<Node>,
    via: Option<&fastctx_types::access::HubId>,
    typed_on: &MachineName,
    to: &ResidentId,
    text: String,
    attachments: Vec<Attachment>,
    files_on: Option<&Written>,
    id: Option<&str>,
) -> Result<Landed, Failure> {
    crate::attachments::resolve(node, to, &attachments).await?;
    let attachments = crate::attachments::place(node, to, attachments, files_on).await?;
    crate::placement::location::speaking(node, to, typed_on)?;
    let id = id.map(|id| speech::talk_id(node, via, typed_on, id));
    let delivery = crate::attachments::delivery(typed_on, text, attachments, id);
    speech::tell_here(node, to, delivery, Owed::Nobody).await
}

/// Watches a resident on any machine. What it hands over comes on the
/// returned channel until the watch ends, with why when something went
/// wrong; dropping the channel ends the watch. A viewer that says what it
/// holds of the history (`held`) is sent it as segments, slimmed.
#[allow(clippy::too_many_arguments)]
pub async fn watch(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
    composer: Option<String>,
    held: Option<Held>,
    now: bool,
    live_only: bool,
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
            opening(node, &resident, composer, held.as_ref(), live_only).await?
        };
        let Opening { pieces, mut events, slim } = opened;
        let node = node.clone();
        fastctx_platform::task::spawn(async move {
            let _lease = lease;
            let mut views = node.views.subscribe();
            for piece in pieces.into_iter().chain(told.first().map(OpeningPiece::Made)) {
                let piece = match piece.made().await {
                    Ok(piece) => piece,
                    Err(failure) => {
                        let _ = sender.send(Err(failure));
                        return;
                    }
                };
                if sender.send_wait(Ok(piece), std::time::Duration::from_secs(30)).await.is_err() {
                    return;
                }
            }
            let mut forwarder = forwarding::Forwarder::new(sender);
            loop {
                tokio::select! {
                    changed = views.changed() => {
                        if changed.is_err() { return; }
                        views.borrow_and_update();
                        if !crate::admission::seen::resident(&node, crate::admission::seen::Looker::Here(&Acting::Here), &resident) {
                            let _ = forwarder.sender().send(Err(Failure::refused("access to this conversation was withdrawn")));
                            return;
                        }
                    },
                    room = forwarder.room(), if forwarder.waiting() => {
                        if room.is_err() || forwarder.flush().is_err() { return }
                    },
                    event = events.recv() => match event {
                        Ok(Some(event)) => {
                            if goal.is_some() && !crate::goals::watch::is_goal(&event) { continue; }
                            if !crate::goals::visible(&node, crate::admission::seen::Looker::Here(&Acting::Here), &resident, &event) { continue; }
                            let marked = Arc::unwrap_or_clone(event);
                            let Some(event) = likeness::watched(marked.event, marked.mark, slim.as_ref(), live_only) else { continue };
                            if forwarder.push(event).is_err() { return }
                        }
                        Ok(None) => return,
                        Err(_) => { forwarder.sender().lag(); return; },
                    },
                    now = told.changed(&node) => {
                        if let Some(now) = now && forwarder.push(now).is_err() { return }
                    }
                    () = forwarder.sender().closed() => return,
                }
            }
        });
        return Ok(receiver);
    }
    let acting =
        goal.as_ref().map(|account| Acting::one(account.hub.clone(), account.account.clone())).unwrap_or(Acting::Here);
    if let Some(account) = &goal
        && !node.serves(&Acting::Here, &account.hub, &account.account)
    {
        return Err(Failure::refused(fastctx_types::goals::words::WATCH_ACCOUNT));
    }
    let request = fastctx_types::watching::Subscription {
        resident,
        composer,
        documents: None,
        held,
        now,
        live_only,
        goal: goal.is_some(),
        collaboration: None,
    };
    let mut watching = None;
    let mut refused = None;
    for (link, account) in node.speakers(&acting, &machine, Purpose::Agents).map_err(|e| Failure::new(e.to_string()))? {
        if live_only {
            // Paged, a conversation is read through calls an earlier hub
            // cannot relay; it is watched whole then (`timeline_pages`).
            if !link.view().timeline_pages {
                refused = Some(Failure::unsupported("this network's hub does not relay timeline pages; update the hub"));
                continue;
            }
            match link.call(&machine.name, &account, Purpose::Look, fastctx_types::remote::RemoteCall::Facts).await {
                Ok(Ok(RemoteReply::Facts(facts))) if facts.timeline_reads && facts.watch_live => {}
                Ok(Err(failure)) => return Err(failure),
                Err(error) => return Err(Failure::new(error.to_string())),
                _ => return Err(Failure::unsupported("this machine does not support live-only watches; update it")),
            }
        }
        match link.watch_shared(&machine.name, &account, request.clone()).await {
            Ok(opened) => {
                watching = Some(opened);
                break;
            }
            Err(failure) if failure.not_run => refused = Some(failure),
            Err(failure) => return Err(failure),
        }
    }
    let Some(mut watching) = watching else {
        return Err(refused.expect("a speaker was tried"));
    };
    fastctx_platform::task::spawn(async move {
        // The remote machine's pieces are taken as they come, so it hears
        // they were taken; what this watcher has no room for waits here,
        // merged, as at the source.
        let mut forwarder = forwarding::Forwarder::new(sender);
        loop {
            let piece = tokio::select! {
                room = forwarder.room(), if forwarder.waiting() => {
                    if room.is_err() || forwarder.flush().is_err() { return }
                    continue;
                }
                piece = watching.next() => piece,
                () = forwarder.sender().closed() => return,
            };
            match piece {
                Ok(Some(piece)) => {
                    if forwarder.push(piece).is_err() {
                        return;
                    }
                }
                Ok(None) => return,
                Err(failure) => {
                    let _ = forwarder.flush();
                    let _ = forwarder.sender().send(Err(failure));
                    return;
                }
            }
        }
    });
    Ok(receiver)
}

/// A piece of an opening: made already, or segments of a history read from
/// where its cut keeps them as the piece is sent (design §28.52 52-1).
pub(crate) enum OpeningPiece {
    Made(Watched),
    Kept { cut: Arc<crate::cut_history::Kept>, planned: fastctx_net::history::Planned },
}

impl OpeningPiece {
    /// The piece, its segments read.
    pub(crate) async fn made(self) -> Result<Watched, Failure> {
        match self {
            OpeningPiece::Made(watched) => Ok(watched),
            OpeningPiece::Kept { cut, planned } => crate::cut_history::made(cut, planned).await,
        }
    }
}

/// What opening a watch of a resident of this machine hands over first,
/// the events after it, and whose they are when they go slimmed.
pub(crate) struct Opening {
    pub pieces: Vec<OpeningPiece>,
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
    live_only: bool,
) -> Result<(Option<crate::composer::Lease>, Opening), Failure> {
    if let Some(name) = composer {
        let lease = crate::composer::watch(node, resident, name).await?;
        let events = node.watchers.watch_composer(resident);
        crate::composer::snapshot(node, resident).await?;
        let pieces = vec![OpeningPiece::Made(Watched::History { events: Vec::new() })];
        return Ok((Some(lease), Opening { pieces, events, slim: None }));
    }
    if live_only {
        let (events, pending) = subscribe(node, resident)?;
        let mut pieces: Vec<_> =
            pending.into_iter().filter_map(|event| likeness::watched(event, None, None, true)).map(OpeningPiece::Made).collect();
        pieces.push(OpeningPiece::Made(Watched::Resumed));
        return Ok((None, Opening { pieces, events, slim: None }));
    }
    if let Some(held) = held {
        return Ok((None, crate::held_history::opening(node, resident, held).await?));
    }
    let (history, events) = watch_here(node, resident).await?;
    Ok((None, Opening { pieces: vec![OpeningPiece::Made(Watched::History { events: history })], events, slim: None }))
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
pub async fn watch_here(node: &Arc<Node>, resident: &ResidentId) -> Result<(Vec<SessionEvent>, Events), Failure> {
    // Read from the conversation's timeline where it is read here, heard
    // first (design §28.52 52-1): its sources are not read whole for it.
    if node.residents.about(resident).is_some_and(|(about, _)| about.world_managed()) {
        let (events, pending) = requested(node, resident, node.watchers.watch(resident))?;
        if let Some(mut history) = crate::timeline::viewed::history(node, resident).await? {
            history.extend(pending);
            return Ok((history, events));
        }
    }
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
    /// Some record the history is read from could not be read now: this is
    /// not the history as its files hold it, and is not to be kept as such.
    pub unread: bool,
    /// The host has not written all it reported keeping yet.
    pub unsettled: bool,
    /// Actual history-read problems, distinct from records still being written.
    pub unavailable: Vec<String>,
    /// A record of it stopped before a line larger than World reads
    /// (§28.49): nothing kept of it is taken away by this read, nor written
    /// again where an event of that record names it (`frozen`, one set for
    /// each record stopped short).
    pub refused: bool,
    pub frozen: Vec<std::collections::HashSet<String>>,
    /// For a read heard apart (`history_apart`): what was under way as its
    /// events began, and the mark of the last event told before them.
    pub snapshots: Vec<Arc<crate::watch::Marked>>,
    pub at: Option<fastctx_types::session::Mark>,
}

/// How a read of a resident's history hears its events from then on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hearing {
    /// As a viewer, what is under way first among them.
    Viewer,
    /// As a viewer, what is under way apart, with where the events begin.
    Apart,
    /// Without viewing it.
    Quiet,
}

/// Starts watching a resident of this machine, without reading its history:
/// the events from now on, and the requests its host waits on someone to
/// answer now.
pub(crate) fn subscribe(node: &Arc<Node>, resident: &ResidentId) -> Result<(Events, Vec<SessionEvent>), Failure> {
    requested(node, resident, node.watchers.watch(resident))
}

/// `events` of a resident of this machine, its host's requests among them
/// passed on only while they wait, and the requests that wait now.
pub(crate) fn requested(
    node: &Arc<Node>,
    resident: &ResidentId,
    events: Events,
) -> Result<(Events, Vec<SessionEvent>), Failure> {
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
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
    read_here(node, resident, Hearing::Viewer).await.map_err(|failure| as_deleted(node, resident, failure))
}

/// A resident's history and its events from then on, read for something
/// other than a viewer, so its host is not kept awake for the reading.
pub(crate) async fn history_quietly(node: &Arc<Node>, resident: &ResidentId) -> Result<HistoryRead, Failure> {
    read_here(node, resident, Hearing::Quiet).await.map_err(|failure| as_deleted(node, resident, failure))
}

/// What `history_quietly` reads of a Claude resident whose host is read,
/// kept on disk as it is read (design §28.51 ⑤-4): the World's record as
/// `read_here` takes it (`recorded`), and the host's history placed in a
/// work store (`claude::history::hosted_kept`), with the objects of its
/// events put as they were read (`early`).
pub(crate) struct HistoryKept {
    pub(crate) hosted: crate::claude::history::HostedKept,
    /// The World's record, kept on disk, its names and keys beside it.
    pub(crate) world: Arc<std::sync::Mutex<crate::timeline::world::World>>,
    pub(crate) early: Option<crate::timeline::Early>,
}

/// `history_quietly` of a Claude resident whose host is read, its history
/// read into `spill` rather than held (`HistoryKept`); none where its host
/// is not read now (`logins::workbench::reads_host`, asked again here as
/// `read_here` asks it), for `history_quietly` to read as the World
/// recorded it.
pub(crate) async fn history_kept(
    node: &Arc<Node>,
    resident: &ResidentId,
    spill: crate::timeline::spill::Spill,
    world: crate::timeline::spill::Spill,
    early: Option<crate::timeline::Early>,
) -> Result<Option<HistoryKept>, Failure> {
    read_kept_here(node, resident, spill, world, early).await.map_err(|failure| as_deleted(node, resident, failure))
}

async fn read_kept_here(
    node: &Arc<Node>,
    resident: &ResidentId,
    spill: crate::timeline::spill::Spill,
    world: crate::timeline::spill::Spill,
    early: Option<crate::timeline::Early>,
) -> Result<Option<HistoryKept>, Failure> {
    let _doing = fastctx_platform::doing::doing(format!("reading {resident}'s history"));
    let (about, _) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
    // As `read_here` begins.
    if !about.world_managed() {
        return Err(Failure::refused(crate::speech::OWN_SESSION));
    }
    if about.host.agent.as_str() != "claude" || !crate::logins::workbench::reads_host(node, &about) {
        return Ok(None);
    }
    let _heard = node.watchers.watch_quietly(resident);
    let directory = node.world.conversation(resident);
    let preparing = directory.clone();
    fastctx_platform::task::spawn_blocking(move || crate::session_record::prepare(&preparing))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(crate::record_migration::failure)?;
    // The World's record kept on disk as it is read (design §28.51 ⑥-1):
    // read from its start, the turns a cut took back left out, as
    // `read_here` leaves them out.
    let reading = directory.clone();
    let world = fastctx_platform::task::spawn_blocking(move || crate::timeline::world::World::read(world, &reading, 0, &[]))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(Failure::new)?;
    let world = Arc::new(std::sync::Mutex::new(world));
    let (hosted, early) = crate::claude::history::hosted_kept(node, resident, world.clone(), directory, spill, early).await.map_err(Failure::new)?;
    Ok(Some(HistoryKept { hosted, world, early }))
}

/// The World's record of a resident's conversation whose host is not read,
/// as `history_quietly` reads it — as the World recorded it, no turn a cut
/// took back left out — but kept in `spill` as it is read, not held
/// (design §28.51 ⑥-4; `World::view_as_recorded` orders it as
/// `recorded_history` does). None where `read_here` would read its host
/// now: its workbench open again since it was asked, say.
pub(crate) async fn recorded_kept(
    node: &Arc<Node>,
    resident: &ResidentId,
    spill: crate::timeline::spill::Spill,
) -> Result<Option<crate::timeline::world::World>, Failure> {
    let read = async {
        let _doing = fastctx_platform::doing::doing(format!("reading {resident}'s history"));
        let (about, _) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
        // As `read_here` begins.
        if !about.world_managed() {
            return Err(Failure::refused(crate::speech::OWN_SESSION));
        }
        let _heard = node.watchers.watch_quietly(resident);
        // As `read_here` decides: a session's host is read where the
        // workbench is open in its login.
        let agent = about.host.agent.as_str();
        if crate::logins::workbench::reads_host(node, &about) && about.session.is_some() && (agent == "codex" || agent == "claude") {
            return Ok(None);
        }
        let directory = node.world.conversation(resident);
        let world = fastctx_platform::task::spawn_blocking(move || crate::timeline::world::World::read_as_recorded(spill, &directory))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(Failure::new)?;
        // As `read_here` asks a Codex thread's host for what waits on an
        // answer once its history is read: its host failing fails it.
        if let Some(thread) = &about.session
            && agent == "codex"
        {
            drop(node.codex.of(node, &about)?.requests(thread));
        }
        Ok(Some(world))
    };
    read.await.map_err(|failure| as_deleted(node, resident, failure))
}

/// A resident's history for a viewer, its events from then on heard apart
/// from what is under way, with where they begin.
pub(crate) async fn history_apart(node: &Arc<Node>, resident: &ResidentId) -> Result<HistoryRead, Failure> {
    read_here(node, resident, Hearing::Apart).await.map_err(|failure| as_deleted(node, resident, failure))
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

async fn read_here(node: &Arc<Node>, resident: &ResidentId, hearing: Hearing) -> Result<HistoryRead, Failure> {
    let _doing = fastctx_platform::doing::doing(format!("reading {resident}'s history"));
    let (about, _) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
    // A session the user opened on their own is theirs to read, in their
    // own program (`world-layers.md`).
    if !about.world_managed() {
        return Err(Failure::refused(crate::speech::OWN_SESSION));
    }
    let (mut events, snapshots, at) = match hearing {
        Hearing::Viewer => (node.watchers.watch(resident), Vec::new(), None),
        Hearing::Quiet => (node.watchers.watch_quietly(resident), Vec::new(), None),
        Hearing::Apart => {
            let followed = node.watchers.follow(resident, None);
            (followed.events, followed.snapshots, Some(followed.at))
        }
    };
    let directory = node.world.conversation(resident);
    let preparing = directory.clone();
    fastctx_platform::task::spawn_blocking(move || crate::session_record::prepare(&preparing))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(crate::record_migration::failure)?;
    async fn recorded(directory: &ConversationDir) -> Result<Vec<SessionEvent>, Failure> {
        let directory = directory.clone();
        fastctx_platform::task::spawn_blocking(move || crate::delivery::history(&directory))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(crate::record_migration::failure)
    }
    let owner = Address { machine: node.machine(), resident: resident.clone() };
    let mut unread = false;
    let mut unsettled = false;
    let mut unavailable = Vec::new();
    let mut refused = false;
    let mut frozen = Vec::new();
    // A session the user opened, or one in a login the workbench is closed
    // in, has its host neither started nor read: the conversation reads as
    // the World recorded it.
    let hosted = crate::logins::workbench::reads_host(node, &about);
    let (recorded, hosted) = match &about.session {
        _ if !hosted => (recorded(&directory).await?, None),
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
            let recorded = crate::codex::watch::recorded(recorded(&directory).await?, &history);
            // A request can arrive and resolve while history is loading.
            // Only currently unanswered requests may enter a new view; the
            // subscription already covers changes after this snapshot.
            events = crate::codex::watch::requests(codex, thread.clone(), events);
            (recorded, Some(history))
        }
        // Claude Code's own record of the session, placed in the turns the
        // World saw, which the World's record says.
        _ if about.host.agent.as_str() == "claude" => {
            let mut recorded = recorded(&directory).await?;
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
            unread = hosted.unread;
            unsettled = hosted.unsettled;
            unavailable = hosted.unavailable;
            refused = hosted.refused;
            frozen = hosted.frozen;
            (recorded, Some(hosted.events))
        }
        _ => (recorded(&directory).await?, None),
    };
    let (working, resident, agent) = (node.clone(), resident.clone(), about.host.agent.to_string());
    let history =
        fastctx_platform::task::spawn_blocking(move || assemble(&working, &resident, &directory, &agent, recorded, hosted))
            .await
            .map_err(|error| Failure::new(error.to_string()))??;
    let pending = match &about.session {
        Some(thread) if about.world_managed() && about.host.agent.as_str() == "codex" => {
            node.codex.of(node, &about)?.requests(thread)
        }
        _ => Vec::new(),
    };
    Ok(HistoryRead { history, pending, events, unread, unsettled, unavailable, refused, frozen, snapshots, at })
}

/// The history of `recorded`, what the World kept, and `hosted`, what its
/// host keeps, as a reader is shown it: the host's items with the context
/// they were given, the attachments delivered, each with the facts of the
/// runs it tells of. Call on a thread that may block.
pub(crate) fn assemble(
    node: &Arc<Node>,
    resident: &ResidentId,
    directory: &ConversationDir,
    agent: &str,
    recorded: Vec<SessionEvent>,
    mut hosted: Option<Vec<SessionEvent>>,
) -> Result<Vec<SessionEvent>, Failure> {
    if let Some(events) = &mut hosted {
        let projection = crate::context::input::Projection::open(directory).map_err(crate::record_migration::failure)?;
        for event in events {
            projection.restore(event).map_err(crate::record_migration::failure)?;
        }
    }
    let receipts = crate::attachments::delivered::of(node, resident);
    let mut history = merged(recorded, hosted, agent, &receipts);
    for event in &mut history {
        crate::run_facts::attach(directory, event).map_err(crate::record_migration::failure)?;
    }
    Ok(history)
}

/// The history the World's record and the host's make together, as
/// `assemble` makes it before the conversation's own files add to it.
pub(crate) fn merged(
    recorded: Vec<SessionEvent>,
    hosted: Option<Vec<SessionEvent>>,
    agent: &str,
    receipts: &dyn fastctx_types::session::attachments::Receipts,
) -> Vec<SessionEvent> {
    let mut history = history(recorded, hosted, agent, receipts);
    history.retain(|event| !changes_history(event));
    history
}

/// Whether `event` tells a change of history: it tells a view already open
/// to read again, and the history read here is already what follows it.
/// Kept in a view, an interface that reads it as news would read again on
/// every event.
pub(crate) fn changes_history(event: &SessionEvent) -> bool {
    matches!(
        &event.happened,
        fastctx_types::session::Happened::Changed {
            change: fastctx_types::session::SessionChange::HistoryChanged | fastctx_types::session::SessionChange::HistoryCut { .. },
            ..
        }
    )
}

/// `assemble` of a history whose host's events are kept on disk (design
/// §28.51): their keys (`hosted_keys`, named by `names`, the context and
/// attachments restored as `shown` shows them), each taken by its index
/// (`hosted`). The World's `recorded` is put among them as `history` puts
/// it, and the history handed on one event at a time (`each`), each with the
/// facts of the runs it tells of (where there are the conversation's files
/// to tell them). Call on a thread that may block.
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble_each(
    directory: Option<&ConversationDir>,
    agent: &str,
    recorded: Recorded<'_>,
    hosted_keys: merging::HostKeys,
    names: &mut merging::Names,
    hosted: &mut dyn FnMut(usize) -> Result<SessionEvent, Failure>,
    each: &mut dyn FnMut(SessionEvent) -> Result<(), Failure>,
) -> Result<(), Failure> {
    let mut each = |mut event: SessionEvent| -> Result<(), Failure> {
        if changes_history(&event) {
            return Ok(());
        }
        if let Some(directory) = directory {
            crate::run_facts::attach(directory, &mut event).map_err(crate::record_migration::failure)?;
        }
        each(event)
    };
    let codex = agent == "codex";
    match recorded {
        Recorded::Held(recorded) => {
            let recorded_keys: Vec<merging::Key> = recorded.iter().map(|event| merging::key(event, names)).collect();
            // Laid out sparsely (design §28.51 ⑤-6): nothing held for each of the
            // host's events but its time, its key and a few places.
            let layout = merging::sparse::decide(hosted_keys, &merging::sparse::Held(&recorded_keys), codex).map_err(Failure::new)?;
            drop(recorded_keys);
            merging::apply_each(layout.slots(), hosted, recorded, &mut each)
        }
        // Its keys kept beside it as the names number them (`World::name`),
        // its events read back in the order laid out (§28.51 ⑥-1).
        Recorded::Kept(world) => {
            let layout = merging::sparse::decide(hosted_keys, &world.keys(), codex).map_err(Failure::new)?;
            world.apply(layout.slots(), hosted, &mut each, Failure::new)
        }
    }
}

/// The World's record a merge lays out with the host's events
/// (`assemble_each`): held, or kept on disk, its view the events merged and
/// their keys kept beside them (`timeline::world`).
pub(crate) enum Recorded<'w> {
    /// Held: as the tests and the bench weigh the store against.
    #[cfg_attr(not(any(test, feature = "bench")), allow(dead_code))]
    Held(Vec<SessionEvent>),
    Kept(&'w crate::timeline::world::World),
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
    let history: Vec<SessionEvent> =
        hosted.into_iter().flat_map(|event| shown(event, receipts, agent, &|turn| sent_turns.contains(turn))).collect();
    // The World's record put among the host's events (`merging`): decided on
    // what each event shows of itself, then laid out.
    let mut names = merging::Names::default();
    let hosted_keys: Vec<merging::Key> = history.iter().map(|event| merging::key(event, &mut names)).collect();
    let recorded_keys: Vec<merging::Key> = recorded.iter().map(|event| merging::key(event, &mut names)).collect();
    let slots = merging::decide(&hosted_keys, &recorded_keys, agent == "codex");
    // The host's canonical timeline is kept, including interleaved voice
    // items and steering input; local facts stand at their source time.
    merging::apply(slots, history, recorded)
}

/// What a reader is shown of the host's `event` in a history (`history`):
/// its attachments as they were delivered, and of words the World told, only
/// those its own record does not tell. `sent` says whether the World's record
/// has words it sent into a turn. One event at a time, so that a reading kept
/// on disk shows each as one held in memory does (design §28.51).
pub(crate) fn shown(
    event: SessionEvent,
    receipts: &dyn fastctx_types::session::attachments::Receipts,
    agent: &str,
    sent: &dyn Fn(&str) -> bool,
) -> Vec<SessionEvent> {
    use fastctx_types::session::Happened;
    let mut restored = fastctx_types::session::attachments::restore(event, receipts);
    restored.retain(|event| match &event.happened {
        Happened::Heard { message: Some(message), .. } if !message.images.is_empty() => true,
        Happened::Heard { delivery: Some(_), message: Some(message), .. } if !message.attachments.is_empty() => true,
        // Other words the World told are its own record's, except, for
        // Codex, in a turn the World has no record of sending into. The
        // World's head note tells them, and in Codex, where the user's
        // words go in as typed, the id the World gave the message.
        Happened::Heard { turn, text, message, .. } => {
            let told = fastctx_hands::told::is_told(text)
                || agent == "codex" && message.as_deref().is_some_and(fastctx_hosts::codex::sent_by_world);
            !(told && (agent != "codex" || turn.as_deref().is_some_and(sent)))
        }
        _ => true,
    });
    restored
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

pub(crate) use fastctx_timeline_core::talk::turn_of;

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
    live_only: bool,
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
            opening(node, &resident, composer, held.as_ref(), live_only).await
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
    let mut views = node.views.subscribe();
    let allowed = || crate::admission::seen::resident(node, crate::admission::seen::Looker::Asker(asker), &resident);
    let mut n = 0;
    let mut next: std::collections::VecDeque<OpeningPiece> = pieces.into();
    next.extend(told.first().map(OpeningPiece::Made));
    loop {
        while let Some(piece) = next.pop_front() {
            let watched = piece.made().await?;
            if !allowed() {
                stream.close().await;
                return Err(Failure::refused("access to this conversation was withdrawn"));
            }
            let piece = Piece { watch, n, watched };
            let sent = if packed {
                // A history's segments take a while to pack (0.4 s for a
                // long Claude session's 50 MB); they are packed off the
                // runtime's threads, which serve everything else here.
                let bytes = match piece.watched {
                    Watched::Live { .. } => fastctx_net::history::pack(&piece),
                    _ => fastctx_platform::task::spawn_blocking(move || fastctx_net::history::pack(&piece))
                        .await
                        .map_err(|error| Failure::new(format!("packing the history: {error}")))?,
                };
                if !allowed() {
                    stream.close().await;
                    return Err(Failure::refused("access to this conversation was withdrawn"));
                }
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
            changed = views.changed() => {
                if changed.is_err() { return ended("the node stopped"); }
                views.borrow_and_update();
                if !allowed() { stream.close().await; return Err(Failure::refused("access to this conversation was withdrawn")); }
            },
            event = events.recv() => match event {
                Ok(Some(event)) => {
                    if (!goal || crate::goals::watch::is_goal(&event)) && crate::goals::visible(node, crate::admission::seen::Looker::Asker(asker), &resident, &event) {
                        let marked = Arc::unwrap_or_clone(event);
                        next.extend(likeness::watched(marked.event, marked.mark, slim.as_ref(), live_only).map(OpeningPiece::Made));
                    }
                },
                Ok(None) => return ended("this machine stopped telling its events"),
                Err(error) => { stream.close().await; return Err(Failure::new(error.to_string())); },
            },
            now = told.changed(node) => next.extend(now.map(OpeningPiece::Made)),
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
    last: Option<NowTold>,
}

/// What a `Watched::Now` told: whether it works, its turn, the words kept,
/// the turn stopped.
type NowTold = (bool, Option<String>, Vec<fastctx_types::session::KeptWords>, Option<String>);

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
    async fn changed(&mut self, node: &Arc<Node>) -> Option<Watched> {
        let Some(changes) = &mut self.changes else { return std::future::pending().await };
        if self.last.is_some() {
            // A change of words kept moves no resident: while some are kept
            // or it works, it is looked at again every few seconds too.
            let unsettled = self.last.as_ref().is_some_and(|(busy, _, kept, _)| *busy || !kept.is_empty());
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
        let (reading, resident) = (node.clone(), self.resident.clone());
        let kept = fastctx_platform::task::blocking(move || kept(&reading, &resident)).await;
        let halted = crate::composer::stopped_turn(node, &self.resident).filter(|halted| turn.as_ref() == Some(halted));
        let state = (busy, turn, kept, halted);
        if self.last.as_ref() == Some(&state) {
            return None;
        }
        self.last = Some(state.clone());
        let (busy, turn, kept, halted) = state;
        Some(Watched::Now { busy, turn, kept, halted })
    }
}

/// How many of the words kept for a resident a view is told of: past this
/// guard, the rest are told as the first go in.
const KEPT_SHOWN: usize = 4096;

/// While a resident works or words are kept for it, how often a watch that
/// tells it looks again.
const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_secs(5);

/// The words kept for `resident` here that no turn has taken yet, and how far
/// each came.
fn kept(node: &Node, resident: &ResidentId) -> Vec<fastctx_types::session::KeptWords> {
    use fastctx_types::residents::PendingState;
    use fastctx_types::session::{KeptWords, WordsState};
    let directory = node.world.conversation(resident);
    let about = node.residents.about(resident).map(|(about, _)| about);
    let managed = about.as_ref().is_some_and(|about| about.world_started);
    let named = about.as_ref().is_some_and(|about| about.session.is_some());
    // A Codex thread being made ready (resumed, its tools started) is its
    // first words' start.
    let mut codex_starting = about.as_ref().filter(|about| about.host.agent.as_str() == "codex").and_then(|about| {
        let thread = about.session.as_deref()?;
        let (phase, since) = node.codex.of(node, about).ok()?.starting(thread)?;
        Some(fastctx_types::session::Starting { since, phase, failures: 0, reason: None, next: None })
    });
    // The first of what waits, as many as a view shows.
    let mut pending = Vec::new();
    let read = crate::inbox::pages(&directory, |page| {
        pending.extend(page);
        Ok(if pending.len() >= KEPT_SHOWN {
            pending.truncate(KEPT_SHOWN);
            std::ops::ControlFlow::Break(())
        } else {
            std::ops::ControlFlow::Continue(())
        })
    });
    if read.is_err() {
        return Vec::new();
    }
    // The words that would start its next turn, while that waits for room
    // in its machine's memory or in its login.
    let mut queued = crate::memory_room::ahead(node, resident)
        .map(|ahead| (ahead, true, false))
        .or_else(|| crate::turn_slots::ahead(node, resident).map(|(ahead, callee)| (ahead, false, callee)));
    // A new conversation's first words wait for memory, or for its login's
    // turn, as they wake it.
    let waits = queued.is_some();
    pending
        .into_iter()
        .map(|pending| {
            let first = pending.delivery.notice.along().is_none()
                && (pending.state == PendingState::Waiting || waits && pending.state == PendingState::Waking);
            let progress = crate::lifecycle::retry::progress_now(node, resident, &pending.delivery.id).ok().flatten();
            let retrying = progress.is_some();
            // Its host is being started: a wake keeps how far that came; a
            // new conversation's first session is launching until Claude
            // Code says which it is, then coming up.
            let starting = progress.as_ref().map(crate::lifecycle::retry::Progress::starting).or_else(|| {
                (pending.state == PendingState::Waking).then(|| fastctx_types::session::Starting {
                    since: pending.since.unwrap_or(pending.delivery.at),
                    phase: if named {
                        fastctx_types::session::StartPhase::Connecting
                    } else {
                        fastctx_types::session::StartPhase::Launching
                    },
                    failures: 0,
                    reason: None,
                    next: None,
                })
            })
            .or_else(|| codex_starting.take());
            // A view from before `starting` is told only of starts that
            // failed, a minute on: what the host said while it came up is no
            // failure.
            let host_retry = progress
                .filter(|progress| progress.failures > 0)
                .filter(|progress| Timestamp::now().as_second().saturating_sub(progress.began.as_second()) >= 60)
                .and_then(|progress| {
                    progress.reason.map(|reason| fastctx_types::session::HostRetry { since: progress.began, reason })
                });
            let (queued, for_memory, for_callee) = match first.then(|| queued.take()).flatten() {
                Some((ahead, memory, callee)) => (Some(ahead), memory, callee),
                None => (None, false, false),
            };
            KeptWords {
                id: pending.delivery.id,
                state: match pending.state {
                    PendingState::Waiting if !retrying && !managed => WordsState::Waiting,
                    _ => WordsState::Handing,
                },
                host_retry,
                queued,
                for_memory,
                for_callee,
                starting,
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
                    node.speaking(&acting, &target, Purpose::Agents).await.map_err(|e| Failure::new(e.to_string()))?;
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
            let asking = crate::submitted::Asking::begin();
            let reply = loop {
                let reply = node
                    .call(&acting, &target, Purpose::Agents, call.clone())
                    .await
                    .map_err(|failure| Failure::new(failure.to_string()))?;
                if let Some(reply) = asking.settled(reply).await {
                    break reply;
                }
            };
            match reply {
                Ok(RemoteReply::Opened { resident, landed }) => {
                    Ok((Address { machine: target.name, resident }, landed))
                }
                Ok(_) => Err(Failure::new(format!("{target} answered something other than an opened session"))),
                // What went wrong there is said of that machine, not this one.
                Err(failure) => Err(Failure { message: format!("on {target}: {}", failure.message), ..failure }),
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
    let mut options = options;
    options.mcp_off = choices.mcp_off;
    let home = {
        // The logins on this machine are read from disk.
        let (listing, agent, chosen) = (node.clone(), agent.clone(), choices.home.clone());
        fastctx_platform::task::blocking(move || crate::homes::listed(&listing, agent.as_str(), chosen.as_deref()))
            .await?
    };
    crate::logins::workbench::may_start(node, &acting, agent.as_str(), home.as_deref())?;
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
    {
        let (placing, conversation, typed_on) = (node.clone(), id.clone(), typed_on.clone());
        fastctx_platform::task::blocking(move || {
            crate::placement::location::speaking(&placing, &conversation, &typed_on)
        })
        .await?;
    }
    let origin = Origin::User;
    let _birth = crate::here::born(node, &id, opened.acting.clone(), origin.clone());
    crate::session_options::keep(node, &id, &options).await?;
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
        born_shared: crate::timeline::born_shared::now(node),
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
    crate::context::instructed(node, &id, &heard).await.map_err(|error| Failure::new(error.to_string()))?;
    node.residents.set_instructed(&id, heard);
    crate::codex::claim(node, &thread, &id).await;
    let landed = talk_here(node, None, typed_on, &id, text, attachments, opened.files_on.as_ref(), None).await?;
    Ok((id, landed))
}

/// Stops the turn a resident on any machine is running; returns where it
/// lives and whether a turn was running.
pub async fn stop(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
    turn: Option<String>,
    words: Option<Vec<fastctx_types::NoticeId>>,
) -> Result<(Address, bool), Failure> {
    let here = node.machine();
    let local = machine.is_none() && node.residents.about(&resident).is_some();
    let machine = match machine {
        Some(machine) => machine,
        None => locate(node, &resident).await?,
    };
    if local || node.is_here(&machine) {
        let stopped = stop_seen(node, &resident, turn.as_deref(), words.as_deref()).await?;
        return Ok((Address { machine: here, resident }, stopped));
    }
    let call = RemoteCall::Stop { resident: resident.clone(), turn, words };
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

/// Stops, for the user, the turn they saw running (`seen`), and no other:
/// one that ended meanwhile is left ended, and a turn begun since, by a
/// notice that came after it, is not theirs to stop. Seeing none, with the
/// words that were on their way when they pressed it (`words`), it stops
/// only the turn that takes those, now or when it starts: words sent after
/// the stop, and whatever else starts a turn, go on as they would. Without
/// either (an interface before them) it stops what runs, as `stop_here`.
pub async fn stop_seen(
    node: &Arc<Node>,
    resident: &ResidentId,
    seen: Option<&str>,
    words: Option<&[fastctx_types::NoticeId]>,
) -> Result<bool, Failure> {
    let words = words.filter(|_| seen.is_none());
    stop_turn_as(node, resident, fastctx_types::turn_evidence::StopSource::User, seen, words).await
}

pub(crate) async fn stop_here_as(
    node: &Arc<Node>,
    resident: &ResidentId,
    source: fastctx_types::turn_evidence::StopSource,
) -> Result<bool, Failure> {
    stop_turn_as(node, resident, source, None, None).await
}

async fn stop_turn_as(
    node: &Arc<Node>,
    resident: &ResidentId,
    source: fastctx_types::turn_evidence::StopSource,
    seen: Option<&str>,
    words: Option<&[fastctx_types::NoticeId]>,
) -> Result<bool, Failure> {
    node.residents.stoppable(resident)?;
    let targeted = node.residents.now(resident).and_then(|now| now.turn);
    if seen.is_some() && targeted.as_deref() != seen {
        return Ok(false);
    }
    // Stopped for words on their way: the turn that takes them, now or when
    // it starts, and no other.
    let targeted = match words {
        None => targeted,
        Some(words) => match stop_words(node, resident, source, words).await? {
            Coming::Turn(turn) => Some(turn),
            Coming::Kept => {
                if source == fastctx_types::turn_evidence::StopSource::User {
                    crate::composer::stop_coming(node, resident);
                }
                pause_goal(node, resident, None, source).await;
                return Ok(true);
            }
            Coming::Nothing => return Ok(false),
        },
    };
    // Why the turn ended is read from this when its end comes: it is kept
    // before the host is asked, and does not wait for the disk.
    if let Some(turn) = &targeted {
        crate::turn_evidence::remember(node, resident, turn, source)
            .await
            .map_err(|error| Failure::new(format!("recording why the turn is stopped: {error}")))?;
    }
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    let targeted = match targeted {
        Some(turn) => Some(turn),
        None => match stop_coming(node, resident, source).await? {
            Coming::Kept => {
                // Words sent from now on wait for the turn the words on their
                // way start, and its end, as they would for a turn stopped
                // while it ran.
                if source == fastctx_types::turn_evidence::StopSource::User {
                    crate::composer::stop_coming(node, resident);
                }
                pause_goal(node, resident, None, source).await;
                return Ok(true);
            }
            Coming::Turn(turn) => {
                crate::turn_evidence::remember(node, resident, &turn, source)
                    .await
                    .map_err(|error| Failure::new(format!("recording why the turn is stopped: {error}")))?;
                Some(turn)
            }
            Coming::Nothing => None,
        },
    };
    if let Some(turn) = &targeted {
        // Turns begin and end holding the resident's speech: the turn is
        // still the one stopped when the interrupt goes, never a later one.
        let speaking = node.residents.speaking(resident).ok_or_else(crate::authorization::no_subject)?;
        let _speaking = speaking.lock().await;
        if node.residents.now(resident).and_then(|now| now.turn).as_deref() != Some(turn.as_str()) {
            return Ok(false);
        }
        // What its run calls wait for stops at once, here and on the machines
        // it works on, not once its host has ended the turn.
        node.interrupts.raise(&Address { machine: node.machine(), resident: resident.clone() });
    }
    // What the user sends from now on is for the turn after this one.
    if let Some(turn) = &targeted {
        crate::composer::stop_asked(node, resident, turn);
    }
    let stopped = match (about.host.agent.as_str(), about.world_managed(), &about.session) {
        ("codex", true, Some(thread)) => match &targeted {
            Some(turn) => node.codex.of(node, &about)?.interrupt_turn(thread, turn).await.map_err(Failure::new),
            None => Ok(false),
        },
        ("claude", _, _) => match &targeted {
            Some(turn) => crate::claude::stop(node, resident, turn),
            None => Ok(false),
        },
        _ => Err(Failure::new(format!("{resident} is a session the World does not hold; stop it where it is open"))),
    };
    // The host is asked first; the goal is paused after, and again from the
    // turn's end as one the user stopped (`turns::finish_at`).
    pause_goal(node, resident, targeted, source).await;
    stopped
}

/// What a stop asked while no turn ran is for.
enum Coming {
    /// Words on their way: the turn they start stops as soon as it is known.
    Kept,
    /// A turn begun since, which took words that were on their way then.
    Turn(String),
    Nothing,
}

/// A stop asked while no turn ran. Words on their way start a turn whose id
/// is not known yet: it stops as soon as it is. What only rides with the
/// next turn starts none, and keeps no stop for the turn someone starts
/// later. A turn begun since is the stop's only when it took words that
/// were on their way then; one of words that came later is not stopped.
async fn stop_coming(
    node: &Arc<Node>,
    resident: &ResidentId,
    source: fastctx_types::turn_evidence::StopSource,
) -> Result<Coming, Failure> {
    let reading = node.world.conversation(resident);
    // The words on their way, oldest first, as many as a stop is kept on:
    // a stop reads a bounded part of the inbox however much waits in it.
    let coming = fastctx_platform::task::spawn_blocking(move || {
        let mut coming = Vec::new();
        // Words past however many notices that only ride with the next turn
        // are still found: a stop that missed them would let them start.
        crate::inbox::pages(&reading, |page| {
            coming.extend(
                page.into_iter()
                    .filter(|pending| pending.delivery.notice.along().is_none())
                    .map(|pending| pending.delivery.id),
            );
            Ok(if coming.len() >= COMING_MOST {
                coming.truncate(COMING_MOST);
                std::ops::ControlFlow::Break(())
            } else {
                std::ops::ControlFlow::Continue(())
            })
        })?;
        Ok::<_, std::io::Error>(coming)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(|error| Failure::new(format!("reading what is on its way: {error}")))?;
    if node.stops.coming(node, resident, coming.clone(), source).await.map_err(|error| Failure::new(error.to_string()))? {
        return Ok(Coming::Kept);
    }
    let Some(turn) = node.residents.now(resident).and_then(|now| now.turn) else { return Ok(Coming::Nothing) };
    Ok(if took(node, resident, &turn, &coming).await? { Coming::Turn(turn) } else { Coming::Nothing })
}

/// A stop of the words that were on their way when the user pressed it
/// (`words`), seeing no turn: every turn that takes one of them, and no
/// other. The turn running now is a target if it took one. Each turn that
/// took one and has not become current yet is stopped as soon as it does.
/// Those no turn has taken keep a stop for whichever takes them, even with
/// another turn running: they may be handed to it and not bound yet, or
/// wait for the one after. One taken by a turn that has ended has nothing
/// left to stop.
async fn stop_words(
    node: &Arc<Node>,
    resident: &ResidentId,
    source: fastctx_types::turn_evidence::StopSource,
    words: &[fastctx_types::NoticeId],
) -> Result<Coming, Failure> {
    let directory = node.world.conversation(resident);
    let looking = words.iter().take(COMING_MOST).cloned().collect::<Vec<_>>();
    let current = node.residents.now(resident).and_then(|now| now.turn);
    let other = current.clone();
    // Each word with the turn that took it, where that turn has not ended.
    let bound = fastctx_platform::task::spawn_blocking(move || {
        let mut bound = Vec::new();
        for id in looking {
            let taken = match crate::delivery::input_turn(&directory, &id)? {
                // Gone from the inbox: taken meanwhile, its turn read again
                // now, or taken back or refused, which no turn will take
                // and keeps no stop.
                None if crate::inbox::read(&directory, &id)?.is_none() => match crate::delivery::input_turn(&directory, &id)? {
                    None => continue,
                    Some(turn) => Some(turn),
                },
                taken => taken,
            };
            match taken {
                None => bound.push((None, id)),
                Some(turn) if Some(&turn) == other.as_ref() => bound.push((Some(turn), id)),
                Some(turn) => {
                    if crate::delivery::terminal(&directory, &turn)?.is_none() {
                        bound.push((Some(turn), id));
                    }
                }
            }
        }
        Ok::<_, std::io::Error>(bound)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(|error| Failure::new(format!("reading which turn took the words: {error}")))?;
    let running = current.filter(|turn| bound.iter().any(|(taken, _)| taken.as_deref() == Some(turn.as_str())));
    let kept = bound.iter().filter(|(taken, _)| taken.is_none() || *taken != running).map(|(_, id)| id.clone()).collect::<Vec<_>>();
    if !kept.is_empty() {
        node.stops.keep_for(node, resident, kept.clone(), source).await.map_err(|error| Failure::new(error.to_string()))?;
    }
    // A turn that took some and is not current yet: stopped once it is.
    let mut coming = std::collections::BTreeMap::<String, Vec<fastctx_types::NoticeId>>::new();
    for (taken, id) in &bound {
        if let Some(turn) = taken.as_ref().filter(|turn| Some(*turn) != running.as_ref()) {
            coming.entry(turn.clone()).or_default().push(id.clone());
        }
    }
    let waiting = !coming.is_empty() || bound.iter().any(|(taken, _)| taken.is_none());
    for (turn, ids) in coming {
        crate::stops::deliver(node, resident, &turn, ids);
    }
    if let Some(turn) = running {
        return Ok(Coming::Turn(turn));
    }
    // Bound meanwhile: a binding is ordered with the stop it reads, so one
    // before the stop was kept shows here, and one after stops its turn
    // itself (`speech::bind`).
    let unbound = bound.iter().filter(|(taken, _)| taken.is_none()).map(|(_, id)| id.clone()).collect::<Vec<_>>();
    match node.residents.now(resident).and_then(|now| now.turn) {
        Some(turn) if !unbound.is_empty() && took(node, resident, &turn, &unbound).await? => Ok(Coming::Turn(turn)),
        _ if waiting => Ok(Coming::Kept),
        _ => Ok(Coming::Nothing),
    }
}

/// Whether `turn` took any of `words`. A turn takes the oldest words on
/// their way: whether it took any of them shows among the first.
async fn took(
    node: &Arc<Node>,
    resident: &ResidentId,
    turn: &str,
    words: &[fastctx_types::NoticeId],
) -> Result<bool, Failure> {
    let directory = node.world.conversation(resident);
    let (looking, words) = (turn.to_string(), words.iter().take(TOOK_LOOKS).cloned().collect::<Vec<_>>());
    fastctx_platform::task::spawn_blocking(move || -> std::io::Result<bool> {
        for id in &words {
            if crate::delivery::input_turn(&directory, id)?.as_deref() == Some(looking.as_str()) {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(|error| Failure::new(format!("reading which words the turn took: {error}")))
}

/// The most words on their way a stop asked while no turn ran is kept on.
const COMING_MOST: usize = 4096;
/// How many of the oldest words on their way are looked at for the turn
/// that took them.
const TOOK_LOOKS: usize = 64;

/// A goal the user stopped a turn of waits for them: paused, once the stop
/// is on its way. Its failure leaves the stop standing and is noted.
async fn pause_goal(
    node: &Arc<Node>,
    resident: &ResidentId,
    turn: Option<String>,
    source: fastctx_types::turn_evidence::StopSource,
) {
    if source != fastctx_types::turn_evidence::StopSource::User {
        return;
    }
    if let Err(error) = crate::goals::holding::stop(node, resident, turn).await {
        node.log.line(format_args!("pausing {resident}'s goal after a stop: {}", error.message));
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
pub(crate) async fn locate(node: &Arc<Node>, id: &ResidentId) -> Result<Written, Failure> {
    if node.residents.about(id).is_some() {
        return Ok(node.machine().into());
    }
    // Not here and no hub to ask: where it is cannot be known now, which
    // says nothing of its being deleted unless it was deleted here.
    if !node.connected_anywhere() {
        if node.residents.deleted(id).is_some() {
            return Err(crate::forget::gone(node, id));
        }
        return Err(Failure::unanswered(format!("{id} is not on {} and no network is there to ask", node.machine())));
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
