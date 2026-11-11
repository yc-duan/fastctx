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
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[path = "timeline_navigation.rs"]
mod navigation;

pub(crate) type Shared = Arc<Handle>;
pub(crate) struct Handle {
    pub searches: crate::timeline_search::Searches,
    pub bodies: Arc<tokio::sync::Semaphore>,
    pub scan_bodies: Arc<tokio::sync::Semaphore>,
    navigation: AtomicU64,
    navigation_changed: tokio::sync::Notify,
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
    pub async fn part(&self, generation: u64, edge: Edge, request: u64) -> Result<Option<Part>, String> {
        let previous = self.navigation.fetch_max(request, Ordering::AcqRel);
        if request < previous {
            return Ok(None);
        }
        if request > previous {
            self.navigation_changed.notify_waiters();
        }
        let changed = self.navigation_changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        for attempt in 0..3 {
            let mut work = self.lock().await.clone();
            let following_revision = work.follow_revision;
            if request != self.navigation.load(Ordering::Acquire) {
                return Ok(None);
            }
            let read = tokio::select! {
                read = work.read_part(generation, edge.clone()) => read?,
                () = &mut changed => return Ok(None),
                () = self.closed() => return Ok(None),
            };
            if !read {
                return Ok(None);
            }
            let mut current = self.lock().await;
            if request != self.navigation.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire) {
                return Ok(None);
            }
            let overtaken = work.state.revision < current.state.revision
                || matches!((work.through, current.through), (Some(read), Some(held)) if read < held);
            if overtaken && attempt < 2 {
                continue;
            }
            if overtaken {
                return Err("the conversation is changing too quickly to finish this navigation; try again".into());
            }
            work.state.inherit_live(&current.state);
            current.state = work.state;
            current.through = work.through;
            current.catch_up = work.catch_up;
            if current.follow_revision == following_revision {
                current.following = work.following;
            }
            return Ok(Some(Part::replacement(generation, current.snapshot()?)));
        }
        unreachable!("bounded navigation attempts return")
    }
    async fn advanced(&self) -> Result<Option<Piece>, String> {
        let mut work = self.lock().await.clone();
        let generation = work.transcript.lock().expect("transcript").generation();
        let navigation = self.navigation.load(Ordering::Acquire);
        let following_revision = work.follow_revision;
        let from = work.through;
        let base = work.state.clone();
        work.reanchored = false;
        work.read_advanced().await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let (mut current, rejected) = loop {
            let candidates = {
                let current = self.lock().await;
                crate::timeline_overlay::candidates(&current.state, &work.state)
            };
            let keys: std::collections::BTreeSet<_> =
                candidates.iter().map(|candidate| candidate.key.clone()).collect();
            let rejected = if !work.reanchored
                && let (Some(from), Some(to)) = (from, work.through)
            {
                tokio::select! {
                    result = tokio::time::timeout_at(deadline, crate::timeline_overlay::rejected(&work.world, work.machine.clone(), work.resident.clone(), &base, &work.state, &candidates, from..to)) => result.map_err(|_| "checking live timeline membership exceeded 30 s")??,
                    () = self.closed() => return Ok(None),
                }
            } else {
                Default::default()
            };
            let current = self.lock().await;
            let now = crate::timeline_overlay::candidates(&current.state, &work.state);
            if now.iter().any(|live| !keys.contains(&live.key)) {
                continue;
            }
            break (current, rejected);
        };
        if self.closed.load(Ordering::Acquire)
            || navigation != self.navigation.load(Ordering::Acquire)
            || current.transcript.lock().expect("transcript").generation() != generation
            || current.follow_revision != following_revision
        {
            return Ok(None);
        }
        let before = current.state.clone();
        let mut inherited = before.clone();
        // Only explicit record facts retire these live identities.
        inherited.retire_keys(&rejected);
        let reset_sequence = work.state.revision < current.state.revision
            || matches!((work.through, current.through), (Some(next), Some(old)) if next < old);
        work.state.inherit_retention(&base, &current.state);
        work.state.inherit_reread(&inherited, reset_sequence);
        let delta = crate::timeline_delta::between(&before, &mut work.state, generation)?;
        work.state.retired.clear();
        current.state = work.state;
        current.through = work.through;
        current.catch_up = false;
        current.objects = delta.pages.objects.clone();
        Ok(Some(Piece::Timeline { delta }))
    }
    fn close(&self) {
        self.searches.close();
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
#[derive(Clone, Default)]
pub(crate) struct Readers(Arc<Mutex<HashMap<u64, Shared>>>);
impl Readers {
    pub async fn run(&self, id: u64, reader: Shared, initial: Option<Watching>, pieces: Sender<Piece>) {
        stream(reader, initial, pieces).await;
        self.close(id);
    }
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

#[derive(Clone)]
pub(crate) struct Reader {
    pub world: World,
    pub resident: ResidentId,
    pub machine: Option<Written>,
    pub transcript: Arc<Mutex<Transcript>>,
    state: timeline_window::Window,
    through: Option<u64>,
    catch_up: bool,
    reanchored: bool,
    mark: Option<Mark>,
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
    through: Option<u64>,
    events: Vec<TimelineEvent>,
    older: Option<TimelinePlace>,
    newer: Option<TimelinePlace>,
}

/// `None` selects the unchanged whole-history Watch path for an older peer.
/// Unmaterialized history opens immediately and joins realtime meanwhile.
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
        state: pending,
        through: None,
        catch_up: false,
        reanchored: false,
        mark: None,
        following: true,
        follow_revision: 0,
        objects: Vec::new(),
        closed: Arc::new(AtomicBool::new(false)),
        closing: Arc::new(tokio::sync::Notify::new()),
    };
    let mut page = match reader.fetch_typed(TimelineFrom::Latest, PAGE_EVENTS).await {
        Ok(page) => page,
        Err(error) if older(&error) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if page.revision.is_some() {
        reader.complete_edge(&mut page, true).await.map_err(NotWatched::from)?;
    }
    reader.through = page.through;
    reader.state.preparing = page.revision.is_none();
    reader.state.replace(page.events, page.revision.unwrap_or(0), page.older.is_some(), false);
    let window = reader.snapshot()?;
    let shared = Arc::new(Handle {
        searches: crate::timeline_search::Searches::default(),
        bodies: Arc::new(tokio::sync::Semaphore::new(1)),
        scan_bodies: Arc::new(tokio::sync::Semaphore::new(1)),
        navigation: AtomicU64::new(0),
        navigation_changed: tokio::sync::Notify::new(),
        closed: reader.closed.clone(),
        closing: reader.closing.clone(),
        reader: tokio::sync::Mutex::new(reader),
    });
    Ok(Some(Opened { reader: shared, watching, window }))
}
fn older(error: &Error) -> bool {
    matches!(error, Error::Call(CallError::Failed(failure)) if failure.not_run && failure.code == Some(fastctx_types::ipc::FailureCode::Unsupported))
}

async fn ready(watching: &mut Watching) -> Result<timeline_window::Window, Error> {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let mut pending = timeline_window::Window::default();
        loop {
            match watching.next().await? {
                Some(Watched::Resumed) => return Ok(pending),
                Some(Watched::Live { mut event, mark: None, likeness }) => {
                    crate::timeline_preview::limit(&mut event);
                    pending.observe_likeness(&event, likeness.map(|value| *value));
                    pending.retired.clear();
                    if pending.pending_over_budget {
                        return Err(Error::Other(
                            "the pending-request snapshot exceeds the conversation window budget".into(),
                        ));
                    }
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
        self.fetch_typed(from, max_events).await.map_err(|e| e.to_string())
    }
    async fn fetch_typed(&self, from: TimelineFrom, max_events: u32) -> Result<Page, Error> {
        let reading = self.world.timeline(
            self.machine.clone(),
            TimelineAsk::Page { resident: self.resident.clone(), from, max_events, max_bytes: PAGE_BYTES },
        );
        let reply = tokio::select! {
            reply = reading => reply?,
            () = until_closed(&self.closed, &self.closing) => return Err(Error::Other("the conversation window closed".into())),
        };
        match reply {
            TimelineTold::Page { revision, events, older, newer, through } => {
                Ok(Page { revision, events, older, newer, through })
            }
            _ => Err(Error::Unexpected("the node did not answer a timeline page".into())),
        }
    }
    fn snapshot(&mut self) -> Result<Window, String> {
        let (events, edges) = self.state.shown()?;
        self.state.retired.clear();
        self.objects = edges.objects.clone();
        self.transcript.lock().expect("transcript").paged(&events, edges)
    }
    async fn read_part(&mut self, generation: u64, edge: Edge) -> Result<bool, String> {
        if !matches!(edge, Edge::Latest | Edge::Position { .. } | Edge::Turn { .. })
            && self.transcript.lock().expect("transcript").generation() != generation
        {
            return Ok(false);
        }
        if let Edge::Position { place } = &edge {
            return self.read_position(place.clone()).await;
        }
        if let Edge::Turn { turn } = &edge {
            return self.read_turn(turn).await;
        }
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
            _ => return Ok(false),
        };
        self.following = matches!(edge, Edge::Latest);
        let latest = matches!(from, TimelineFrom::Latest);
        let mut page = self.fetch(from).await?;
        if !latest && (self.catch_up || page.revision != Some(self.state.revision)) {
            self.refresh().await?;
            let place = if backwards { self.state.first() } else { self.state.last() };
            let Some(place) = place else {
                return Ok(true);
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
        self.complete_edge(&mut page, backwards || latest).await?;
        self.through = page.through;
        self.catch_up = false;
        if latest {
            self.state.interval_start = None;
            self.state.interval_end = None;
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
                    self.state.right_complete = next.events.first().is_some_and(|event| {
                        crate::timeline_groups::boundary(timeline_window::decoded(event).as_ref())
                    });
                    self.state.right = next.events.first().map(|e| e.place.clone());
                }
            }
        }
        Ok(true)
    }
    /// Reconcile a lost changes cursor, or navigation across revisions, by
    /// rereading only the held neighborhood.
    async fn refresh(&mut self) -> Result<(), String> {
        self.reanchored = true;
        if !self.following
            && self.state.right.is_none()
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
            } else {
                self.state.newer = false;
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
        self.through = first.through;
        self.catch_up = false;
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
        self.state.preparing = false;
        self.state.replace(events, revision, older, self.state.newer);
        Ok(())
    }
    async fn read_advanced(&mut self) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let Some(after) = self.through else { return self.refresh().await };
            let reading = self.world.timeline(
                self.machine.clone(),
                TimelineAsk::Changes {
                    resident: self.resident.clone(),
                    after,
                    max_events: PAGE_EVENTS,
                    max_bytes: PAGE_BYTES,
                },
            );
            let reply = tokio::select! {
                reply = tokio::time::timeout_at(deadline, reading) => reply.map_err(|_| "catching up the timeline exceeded 30 s")?.map_err(|e| e.to_string())?,
                () = until_closed(&self.closed, &self.closing) => return Err("the conversation window closed".into()),
            };
            match reply {
                TimelineTold::Changes { revision: Some(revision), changes, through, more }
                    if revision >= self.state.revision =>
                {
                    if through < after || (more && through == after) {
                        return Err("the timeline changes did not advance".into());
                    }
                    crate::timeline_changes::apply(&mut self.state, changes, self.following);
                    self.state.revision = revision;
                    self.state.preparing = false;
                    self.through = Some(through);
                    if !more {
                        self.catch_up = false;
                        return Ok(());
                    }
                }
                TimelineTold::Reread | TimelineTold::Changes { revision: Some(_), .. } => return self.refresh().await,
                _ => return Err("the timeline changes are not available now".into()),
            }
        }
    }
    fn live(
        &mut self,
        mut event: SessionEvent,
        mark: Option<Mark>,
        likeness: Option<[u8; 32]>,
    ) -> Result<Option<Piece>, String> {
        if mark.is_some_and(|mark| {
            self.mark.is_some_and(|held| mark.epoch == held.epoch && mark.sequence <= held.sequence)
        }) {
            return Ok(None);
        }
        if let Some(mark) = mark {
            self.mark = Some(mark);
        }
        crate::timeline_preview::limit(&mut event);
        if !timeline_window::pending_event(&event)
            && serde_json::to_vec(&event).map_or(0, |v| v.len()) > crate::timeline_preview::LIVE_EVENT_BYTES
        {
            // An unbounded identity cannot be shortened into another event.
            // Its materialized page supplies the deferred-content placeholder.
            return Ok(None);
        }
        if matches!(event.happened, fastctx_types::session::Happened::Grew { .. })
            && self.state.running(&event).is_none()
            && !self
                .state
                .entries
                .iter()
                .filter_map(|entry| entry.event.as_ref())
                .any(|held| timeline_window::covers(held, &event))
        {
            return Err("waiting for the running item's complete snapshot".into());
        }
        let aliases = self.state.aliases(&event);
        if !self.state.observe_likeness(&event, likeness) {
            if self.state.pending_over_budget {
                return Err("the pending-request snapshot exceeds the conversation window budget".into());
            }
            return Ok(None);
        }
        let pending = timeline_window::pending_event(&event);
        if self.state.newer && !pending {
            return Ok(None);
        }
        if self.state.previewed
            && let Some(mut snapshot) = self.state.running(&event)
        {
            if let fastctx_types::session::Happened::Began { turn, item } = snapshot.happened {
                snapshot.happened = fastctx_types::session::Happened::Updated { turn, item };
            }
            event = snapshot;
        }
        self.state.previewed = false;
        let key = self.state.received_key.clone().unwrap_or_else(|| timeline_window::key(&event));
        let mut evicted = std::mem::take(&mut self.state.retired);
        if !self.state.newer {
            evicted.extend(self.state.trim_visible());
        }
        evicted.extend(aliases.iter().cloned());
        let retained = self.state.ordered_keys();
        evicted.retain(|old| old != &key && !retained.contains(old));
        let anonymous_after = self.state.anonymous.iter().find(|live| live.key == key).map(|live| live.after.clone());
        let placement = if let Some(after) = anonymous_after {
            Some(crate::timeline_delta::Placement { key: key.clone(), after })
        } else if aliases.is_empty() {
            None
        } else {
            let order = self.state.ordered_keys();
            order.iter().position(|held| held == &key).map(|at| crate::timeline_delta::Placement {
                key: key.clone(),
                after: at.checked_sub(1).map(|at| order[at].clone()),
            })
        };
        self.objects.retain(|o| self.state.object_visible(&o.object));
        if let Some(live) = self.state.anonymous.iter_mut().find(|live| live.key == key) {
            live.published = true;
        }
        let generation = self.transcript.lock().expect("transcript").generation();
        Ok(Some(Piece::Live {
            generation,
            key,
            older: self.state.older,
            placement,
            evicted,
            event: serde_json::value::to_raw_value(&event).map_err(|e| e.to_string())?,
        }))
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
                    {
                        let mut current = reader.lock().await;
                        let before = current.state.clone();
                        current.state.pending(pending.pending_events());
                        current.state.retain_live(|event| {
                            !matches!(
                                event.happened,
                                fastctx_types::session::Happened::Began { .. }
                                    | fastctx_types::session::Happened::Updated { .. }
                            )
                        });
                        for event in pending.live {
                            current.state.observe(&event);
                        }
                        current.catch_up = true;
                        let generation = current.transcript.lock().expect("transcript").generation();
                        let mut delta = crate::timeline_delta::between(&before, &mut current.state, generation)
                            .map_err(Error::Other)?;
                        delta.pending = Some(current.state.pending_events());
                        current.objects = delta.pages.objects.clone();
                        pieces.send(Piece::Timeline { delta }).map_err(|_| Error::Other("the page closed".into()))?;
                    }

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
        let (after, catch_up) = {
            let current = reader.lock().await;
            (current.state.revision, current.catch_up)
        };
        let moved = world.timeline(machine.clone(), TimelineAsk::Moved { resident: resident.clone(), after });
        tokio::pin!(moved);
        let mut advancing = catch_up.then(|| Box::pin(reader.advanced()));
        loop {
            tokio::select! {
                () = reader.closed() => return Ok(Follow::Closed),
                result = &mut moved, if advancing.is_none() => {
                    match result? {
                        TimelineTold::Moved { revision: Some(revision) } if revision != after => { advancing = Some(Box::pin(reader.advanced())); },
                        TimelineTold::Moved { .. } => break,
                        _ => return Err(Error::Unexpected("a timeline wait returned another answer".into())),
                    }
                },
                result = async { advancing.as_mut().expect("guarded").await }, if advancing.is_some() => {
                    if let Some(piece) = result.map_err(Error::Other)? { send(piece)?; }
                    break;
                },
                live = watching.next() => { if let Some(follow) = received(reader, pieces, live?).await? { return Ok(follow); } },
            }
        }
    }
}
async fn received(reader: &Shared, pieces: &Sender<Piece>, live: Option<Watched>) -> Result<Option<Follow>, Error> {
    let send = |piece| pieces.send(piece).map_err(|_| Error::Other("the page closed".into()));
    match live {
        Some(Watched::Live { event, mark, likeness }) => {
            if let fastctx_types::session::Happened::Changed { change, .. } = &event.happened {
                use fastctx_types::session::SessionChange;
                match change {
                    SessionChange::HistoryChanged => {
                        reader.lock().await.state.retain_live(|_| false);
                        return Ok(Some(Follow::Refresh));
                    }
                    SessionChange::HistoryCut { removed_turns } => {
                        reader
                            .lock()
                            .await
                            .state
                            .retain_live(|e| !removed_turns.iter().any(|turn| event_turn(e) == Some(turn.as_str())));
                        return Ok(Some(Follow::Refresh));
                    }
                    _ => {}
                }
            }
            if let Some(piece) =
                reader.lock().await.live(event, mark, likeness.map(|value| *value)).map_err(Error::Other)?
            {
                send(piece)?;
            }
        }
        Some(Watched::Now { busy, turn, kept, .. }) => send(Piece::Now { busy, turn, kept })?,
        Some(Watched::Resumed) => {}
        Some(_) => return Err(Error::Unexpected("a live-only subscription sent history".into())),
        None => return Err(Error::Other("the live subscription ended".into())),
    }
    Ok(None)
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
