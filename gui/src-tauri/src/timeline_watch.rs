//! Timeline-backed watches keep only the reader's neighborhood. Realtime is
//! registered first; history is never requested on this subscription.

use crate::{
    NotWatched, Piece, timeline_window,
    transcript::{Edge, Part, Transcript, Window},
    watch_delivery::Sender,
};
use fastctx_client::{CallError, Error, Watching, World};
use fastctx_types::{
    ResidentId,
    access::Written,
    session::{Mark, SessionEvent, Watched},
    timeline_pages::{PAGE_BYTES, PAGE_EVENTS, TimelineAsk, TimelineEvent, TimelineFrom, TimelinePlace, TimelineTold},
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) type Shared = Arc<Handle>;
pub(crate) struct Handle {
    pub bodies: Arc<tokio::sync::Semaphore>,
    reader: tokio::sync::Mutex<Reader>,
    closed: Arc<AtomicBool>,
    closing: Arc<tokio::sync::Notify>,
}
impl std::ops::Deref for Handle {
    type Target = tokio::sync::Mutex<Reader>;
    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}
impl Handle {
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.closing.notify_waiters();
    }
    pub async fn closed(&self) {
        until_closed(&self.closed, &self.closing).await;
    }
}
async fn until_closed(closed: &AtomicBool, closing: &tokio::sync::Notify) {
    let notified = closing.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    if !closed.load(Ordering::Acquire) {
        notified.await;
    }
}
#[derive(Default)]
pub(crate) struct Readers(Mutex<HashMap<u64, Shared>>);
impl Readers {
    pub fn insert(&self, id: u64, reader: Shared) {
        self.0.lock().expect("timeline readers").insert(id, reader);
    }
    pub fn get(&self, id: u64) -> Option<Shared> {
        self.0.lock().expect("timeline readers").get(&id).cloned()
    }
    pub fn close(&self, id: u64) {
        if let Some(reader) = self.0.lock().expect("timeline readers").remove(&id) {
            reader.close();
        }
    }
    pub fn clear(&self) {
        for (_, reader) in self.0.lock().expect("timeline readers").drain() {
            reader.close();
        }
    }
}

pub(crate) struct Reader {
    pub world: World,
    pub resident: ResidentId,
    pub machine: Option<Written>,
    pub transcript: Arc<Mutex<Transcript>>,
    state: timeline_window::Window,
    mark: Option<Mark>,
    emitted: usize,
    emitted_bytes: usize,
    following: bool,
    follow_revision: u64,
    closed: Arc<AtomicBool>,
    closing: Arc<tokio::sync::Notify>,
    pub objects: Vec<timeline_window::Object>,
}
pub(crate) struct Opened {
    pub reader: Shared,
    pub watching: Watching,
    pub window: Window,
}
struct Page {
    revision: Option<u64>,
    events: Vec<TimelineEvent>,
    older: Option<TimelinePlace>,
    newer: Option<TimelinePlace>,
}

/// `None` selects the unchanged whole-history Watch path: an older peer, or
/// a timeline still unmaterialized after its first long wait.
pub(crate) async fn open(
    world: &World,
    resident: ResidentId,
    machine: Option<Written>,
) -> Result<Option<Opened>, NotWatched> {
    let mut watching = match world.watch_live(resident.clone(), machine.clone()).await {
        Ok(watching) => watching,
        Err(error) if older(&error) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let pending = ready(&mut watching).await?;
    let (transcript, _) = Transcript::new(&[])?;
    let mut reader = Reader {
        world: world.clone(),
        resident,
        machine,
        transcript: Arc::new(Mutex::new(transcript)),
        state: timeline_window::Window::default(),
        mark: None,
        emitted: 0,
        emitted_bytes: 0,
        following: true,
        follow_revision: 0,
        objects: Vec::new(),
        closed: Arc::new(AtomicBool::new(false)),
        closing: Arc::new(tokio::sync::Notify::new()),
    };
    reader.state.pending(pending);
    let mut page = reader.fetch(TimelineFrom::Latest).await?;
    if page.revision.is_none() {
        reader
            .world
            .timeline(reader.machine.clone(), TimelineAsk::Moved { resident: reader.resident.clone(), after: 0 })
            .await?;
        page = reader.fetch(TimelineFrom::Latest).await?;
        if page.revision.is_none() {
            return Ok(None);
        }
    }
    reader.state.replace(page.events, page.revision.expect("materialized"), page.older.is_some(), false);
    let window = reader.snapshot()?;
    let shared = Arc::new(Handle {
        bodies: Arc::new(tokio::sync::Semaphore::new(1)),
        closed: reader.closed.clone(),
        closing: reader.closing.clone(),
        reader: tokio::sync::Mutex::new(reader),
    });
    Ok(Some(Opened { reader: shared, watching, window }))
}
fn older(error: &Error) -> bool {
    matches!(error, Error::Call(CallError::Failed(failure)) if failure.not_run && failure.code == Some(fastctx_types::ipc::FailureCode::Unsupported))
}

async fn ready(watching: &mut Watching) -> Result<Vec<SessionEvent>, Error> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let mut pending = Vec::new();
        let mut bytes = 0;
        loop {
            match watching.next().await? {
                Some(Watched::Resumed) => return Ok(pending),
                Some(Watched::Live { event, mark: None }) => {
                    bytes += serde_json::to_vec(&event).map_or(0, |v| v.len());
                    if pending.len() >= timeline_window::HELD_EVENTS || bytes > timeline_window::HELD_BYTES {
                        return Err(Error::Other(
                            "the pending-request snapshot exceeds the conversation window budget".into(),
                        ));
                    }
                    pending.push(event);
                }
                Some(_) => {
                    return Err(Error::Unexpected("a live-only subscription did not acknowledge registration".into()));
                }
                None => return Err(Error::Other("the live subscription ended before registration".into())),
            }
        }
    })
    .await
    .map_err(|_| Error::Other("the live subscription was not acknowledged within 30 s".into()))?
}

impl Reader {
    pub fn follow(&mut self, revision: u64, following: bool) {
        if revision > self.follow_revision {
            self.follow_revision = revision;
            self.following = following;
        }
    }

    async fn fetch(&self, from: TimelineFrom) -> Result<Page, String> {
        self.fetch_count(from, PAGE_EVENTS).await
    }
    async fn fetch_count(&self, from: TimelineFrom, max_events: u32) -> Result<Page, String> {
        let reading = self.world.timeline(
            self.machine.clone(),
            TimelineAsk::Page { resident: self.resident.clone(), from, max_events, max_bytes: PAGE_BYTES },
        );
        let reply = tokio::select! {
            reply = reading => reply.map_err(|e| e.to_string())?,
            () = until_closed(&self.closed, &self.closing) => return Err("the conversation window closed".into()),
        };
        match reply {
            TimelineTold::Page { revision, events, older, newer } => Ok(Page { revision, events, older, newer }),
            _ => Err("the node did not answer a timeline page".into()),
        }
    }
    fn snapshot(&mut self) -> Result<Window, String> {
        let (events, edges) = self.state.shown()?;
        self.emitted = events.len();
        self.emitted_bytes = events.iter().map(|e| serde_json::to_vec(e).map_or(0, |v| v.len())).sum();
        self.objects = edges.objects.clone();
        self.transcript.lock().expect("transcript").paged(&events, edges)
    }
    pub async fn part(&mut self, generation: u64, edge: Edge) -> Result<Option<Part>, String> {
        let previous = self.state.clone();
        match self.read_part(generation, edge).await {
            Ok(part) => Ok(part),
            Err(error) => {
                self.state = previous;
                Err(error)
            }
        }
    }
    async fn read_part(&mut self, generation: u64, edge: Edge) -> Result<Option<Part>, String> {
        if self.transcript.lock().expect("transcript").generation() != generation {
            return Ok(None);
        }
        self.following = matches!(edge, Edge::Latest);
        let backwards = matches!(edge, Edge::Before { .. });
        let from = match edge {
            Edge::Latest => TimelineFrom::Latest,
            Edge::Before { .. } if self.state.older => match self.state.first() {
                Some(place) => TimelineFrom::Before { place },
                None => TimelineFrom::Latest,
            },
            Edge::After { .. } if self.state.newer => match self.state.last() {
                Some(place) => TimelineFrom::After { place },
                None => TimelineFrom::Latest,
            },
            _ => return Ok(None),
        };
        let latest = matches!(from, TimelineFrom::Latest);
        let mut page = self.fetch(from).await?;
        if !latest && page.revision != Some(self.state.revision) {
            self.refresh().await?;
            let place = if backwards { self.state.first() } else { self.state.last() };
            let Some(place) = place else {
                return Ok(Some(Part::replacement(generation, self.snapshot()?)));
            };
            page = self
                .fetch(if backwards { TimelineFrom::Before { place } } else { TimelineFrom::After { place } })
                .await?;
            if page.revision != Some(self.state.revision) {
                return Err("the timeline changed while its window was being read".into());
            }
        }
        let Some(revision) = page.revision else {
            return Err("the conversation's timeline is being prepared".into());
        };
        if latest {
            self.state.right = None;
            self.state.replace(page.events, revision, page.older.is_some(), false);
        } else {
            self.state.revision = self.state.revision.max(revision);
            if backwards {
                self.state.older = page.older.is_some();
            } else {
                self.state.newer = page.newer.is_some();
            }
            self.state.merge(page.events, backwards);
            if !backwards {
                self.state.right = None;
                if self.state.newer
                    && let Some(place) = self.state.last()
                {
                    let next = self.fetch_count(TimelineFrom::After { place }, 1).await?;
                    if next.revision != Some(revision) {
                        return Err("the timeline changed while its edge was being read".into());
                    }
                    self.state.right = next.events.first().map(|e| e.place.clone());
                }
            }
        }
        Ok(Some(Part::replacement(generation, self.snapshot()?)))
    }
    /// A revision has no changed-range list. Reread the held neighborhood so
    /// removals and edits in its middle cannot survive an append-only merge.
    async fn refresh(&mut self) -> Result<(), String> {
        if !self.following
            && !self.state.newer
            && let Some(place) = self.state.last()
        {
            let mut next = self.fetch_count(TimelineFrom::After { place: place.clone() }, 1).await?;
            // Rewriting the anchor changes its item hash at the same order.
            // Keep that replacement inside the frozen window.
            if let Some(first) = next.events.first().filter(|first| first.place.order == place.order) {
                next = self.fetch_count(TimelineFrom::After { place: first.place.clone() }, 1).await?;
            }
            if let Some(first) = next.events.first() {
                self.state.right = Some(first.place.clone());
                self.state.newer = true;
            }
        }
        let wanted = self.state.entries.len().max(1);
        let from = if self.state.newer {
            self.state.right.clone().map(|place| TimelineFrom::Before { place }).unwrap_or(TimelineFrom::Latest)
        } else {
            TimelineFrom::Latest
        };
        let first = self.fetch_count(from, (wanted as u32).min(PAGE_EVENTS)).await?;
        let Some(revision) = first.revision else {
            return Err("the conversation's timeline is being prepared".into());
        };
        let mut events = first.events;
        let mut bytes: usize = events.iter().map(|e| serde_json::to_vec(e).map_or(0, |v| v.len())).sum();
        let mut older = first.older.is_some();
        while wanted > events.len() && older && bytes < timeline_window::HELD_BYTES {
            let Some(place) = events.first().map(|e| e.place.clone()) else { break };
            let mut before = self
                .fetch_count(TimelineFrom::Before { place }, ((wanted - events.len()) as u32).min(PAGE_EVENTS))
                .await?;
            if before.revision != Some(revision) {
                return Err("the timeline changed while its window was being read".into());
            }
            older = before.older.is_some();
            if before.events.is_empty() {
                break;
            }
            bytes += before.events.iter().map(|e| serde_json::to_vec(e).map_or(0, |v| v.len())).sum::<usize>();
            before.events.extend(events);
            events = before.events;
        }
        self.state.replace(events, revision, older, self.state.newer);
        Ok(())
    }
    async fn advanced(&mut self) -> Result<Window, String> {
        let previous = self.state.clone();
        match self.read_advanced().await {
            Ok(window) => Ok(window),
            Err(error) => {
                self.state = previous;
                Err(error)
            }
        }
    }
    async fn read_advanced(&mut self) -> Result<Window, String> {
        if self.following
            && !self.state.newer
            && let Some(place) = self.state.last()
        {
            let page = self.fetch(TimelineFrom::After { place }).await?;
            if let Some(revision) = page.revision {
                self.state.revision = revision;
                self.state.merge(page.events, false);
            }
        }
        self.refresh().await?;
        self.snapshot()
    }
    fn live(&mut self, mut event: SessionEvent, mark: Option<Mark>) -> Result<Option<Piece>, String> {
        if mark.is_some_and(|mark| {
            self.mark.is_some_and(|held| mark.epoch == held.epoch && mark.sequence <= held.sequence)
        }) {
            return Ok(None);
        }
        if let Some(mark) = mark {
            self.mark = Some(mark);
        }
        let previewed = crate::timeline_preview::limit(&mut event);
        if !self.state.observe(&event) {
            if self.state.pending_over_budget {
                return Err("the pending-request snapshot exceeds the conversation window budget".into());
            }
            return Ok(None);
        }
        if self.state.newer {
            self.state.shown()?;
            return Ok(None);
        }
        let bytes = serde_json::to_vec(&event).map_or(0, |v| v.len());
        if previewed
            || self.state.previewed
            || self.state.chunks >= 128
            || self.emitted >= timeline_window::HELD_EVENTS
            || self.emitted_bytes + bytes > timeline_window::HELD_BYTES
            || bytes > 64 << 10
        {
            return Ok(Some(Piece::Window { window: self.snapshot()? }));
        }
        self.emitted += 1;
        self.emitted_bytes += bytes;
        self.transcript.lock().expect("transcript").live_event(&event).map(Some)
    }
}

pub(crate) async fn stream(reader: Shared, mut initial: Option<Watching>, pieces: Sender<Piece>) {
    let (world, resident, machine) = {
        let r = reader.lock().await;
        (r.world.clone(), r.resident.clone(), r.machine.clone())
    };
    let mut again = fastctx_client::Again::default();
    loop {
        let opened = match initial.take() {
            Some(watching) => Ok(watching),
            None => {
                async {
                    let mut watching = world.watch_live(resident.clone(), machine.clone()).await?;
                    let pending = ready(&mut watching).await?;
                    reader.lock().await.state.pending(pending);
                    let window = reader.lock().await.advanced().await.map_err(Error::Other)?;
                    pieces.send(Piece::Window { window }).map_err(|_| Error::Other("the page closed".into()))?;
                    Ok(watching)
                }
                .await
            }
        };
        let result = match opened {
            Ok(watching) => {
                again.came();
                connected(&reader, &world, &resident, &machine, watching, &pieces).await
            }
            Err(error) => Err(error),
        };
        let error = match result {
            Ok(Follow::Closed) => return,
            Ok(Follow::Refresh) => continue,
            Err(error) => error,
        };
        if older(&error) {
            let _ = pieces.send(Piece::Reopen);
            return;
        }
        if error.is_deleted() || error.is_refused() {
            let _ = pieces.send(Piece::failed(&error));
            return;
        }
        if pieces.send(Piece::interrupted(&again.broke(error.to_string()))).is_err() {
            return;
        }
        tokio::select! { () = reader.closed() => return, () = again.pause(&world, machine.as_ref()) => {} }
    }
}

enum Follow {
    Closed,
    Refresh,
}

async fn connected(
    reader: &Shared,
    world: &World,
    resident: &ResidentId,
    machine: &Option<Written>,
    mut watching: Watching,
    pieces: &Sender<Piece>,
) -> Result<Follow, Error> {
    let send = |piece| pieces.send(piece).map_err(|_| Error::Other("the page closed".into()));
    send(Piece::Resumed)?;
    loop {
        let after = reader.lock().await.state.revision;
        let moved = world.timeline(machine.clone(), TimelineAsk::Moved { resident: resident.clone(), after });
        tokio::pin!(moved);
        loop {
            tokio::select! {
                () = reader.closed() => return Ok(Follow::Closed),
                result = &mut moved => {
                    match result? {
                        TimelineTold::Moved { revision: Some(revision) } if revision > after => {
                            let window = reader.lock().await.advanced().await.map_err(Error::Other)?;
                            send(Piece::Window { window })?;
                        },
                        TimelineTold::Moved { .. } => {},
                        _ => return Err(Error::Unexpected("a timeline wait returned another answer".into())),
                    }
                    break;
                },
                live = watching.next() => match live? {
                    Some(Watched::Live { event, mark }) => {
                        if let fastctx_types::session::Happened::Changed { change, .. } = &event.happened {
                            use fastctx_types::session::SessionChange;
                            match change {
                                SessionChange::HistoryChanged => { reader.lock().await.state.live.clear(); return Ok(Follow::Refresh); },
                                SessionChange::HistoryCut { removed_turns } => {
                                    reader.lock().await.state.live.retain(|e| !removed_turns.iter().any(|turn| event_turn(e) == Some(turn.as_str())));
                                    return Ok(Follow::Refresh);
                                },
                                _ => {},
                            }
                        }
                        if let Some(piece) = reader.lock().await.live(event, mark).map_err(Error::Other)? { send(piece)?; }
                    },
                    Some(Watched::Now { busy, turn, kept, .. }) => send(Piece::Now { busy, turn, kept })?,
                    Some(Watched::Resumed) => {},
                    Some(_) => return Err(Error::Unexpected("a live-only subscription sent history".into())),
                    None => return Err(Error::Other("the live subscription ended".into())),
                },
            }
        }
    }
}

fn event_turn(event: &SessionEvent) -> Option<&str> {
    use fastctx_types::session::Happened;
    match &event.happened {
        Happened::TurnStarted { turn }
        | Happened::TurnEnded { turn, .. }
        | Happened::Began { turn, .. }
        | Happened::Updated { turn, .. }
        | Happened::Finished { turn, .. }
        | Happened::Grew { turn, .. }
        | Happened::Steps { turn, .. } => Some(turn),
        Happened::Heard { turn, .. } | Happened::Changed { turn, .. } | Happened::Health { turn, .. } => {
            turn.as_deref()
        }
        _ => None,
    }
}
