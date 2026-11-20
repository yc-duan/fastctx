//! A conversation shown from the copy of it this computer holds. What the
//! page shows is read from the conversation's timeline as the node answers
//! it — from its own copy, or from a machine that holds one where it has
//! none — a part at a time around the reader, and kept current by the
//! records the timeline gains. Nothing that happens live is shown as
//! content: an item is on the page once its record is in the timeline. From
//! the live watch come only what is no content: whether the conversation
//! works now, the words its machine keeps on their way in, and the requests
//! that wait for the user.
//!
//! The page holds the parts it read; nothing of the history is held here.
//! A part begins where a stretch of the conversation begins and ends where
//! one ends, so that what one part shows as a group no neighbour shows the
//! rest of as another.

use crate::watch_delivery::Sender;
use crate::{NotWatched, Piece};
use fastctx_client::{CallError, Error, Watching, World};
use fastctx_types::ResidentId;
use fastctx_types::access::Written;
use fastctx_types::session::{Happened, Item, SessionChange, SessionEvent, Watched, What};
use fastctx_types::timeline_pages::{
    PAGE_BYTES, PAGE_EVENTS, TimelineAsk, TimelineChange, TimelineEvent, TimelineFrom, TimelinePlace, TimelineShown,
    TimelineTold,
};
use serde::Serialize;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long one reading of the timeline may take.
const READ_WITHIN: Duration = Duration::from_secs(30);
/// How long a reading that failed waits before the next, at first and at
/// most.
const AGAIN_FIRST: Duration = Duration::from_millis(500);
const AGAIN_MOST: Duration = Duration::from_secs(10);
/// How long a live watch opened again quietly may take before the page is
/// told it was interrupted, and how often one may end quietly.
const QUIET: Duration = Duration::from_millis(1500);
const QUIET_EVERY: Duration = Duration::from_secs(10);

/// One event of the timeline as the page shows it.
#[derive(Clone, Serialize)]
pub(crate) struct Shown {
    /// The item it is, which names it for as long as the timeline holds it.
    pub key: String,
    pub place: TimelinePlace,
    pub event: Box<RawValue>,
    /// What it holds whole when the event here is cut short.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred: Option<Deferred>,
}

/// An event kept whole as an object, too large for a page.
#[derive(Clone, Serialize)]
pub(crate) struct Deferred {
    pub bytes: u64,
    pub object: String,
}

/// A part of the timeline: its events in order, and where to read on from
/// either way, none where the timeline ends.
#[derive(Clone, Serialize)]
pub(crate) struct Part {
    pub generation: u64,
    pub events: Vec<Shown>,
    pub older: Option<TimelinePlace>,
    pub newer: Option<TimelinePlace>,
    /// Whether the timeline is still being made from the history.
    pub preparing: bool,
    /// Revision and cursor of the same reading, kept as text for JavaScript.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stamp: Option<String>,
}

/// A record the timeline gained: an event as it stands from then on, or the
/// end of one.
#[derive(Clone, Serialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub(crate) enum Record {
    Is { shown: Shown },
    Gone { key: String },
}

/// Where a page of the timeline is read from.
#[derive(Clone, serde::Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub(crate) enum From {
    Latest,
    Before { place: TimelinePlace },
    After { place: TimelinePlace },
}

/// What reads one conversation's timeline for a page.
pub(crate) struct Reader {
    pub world: World,
    pub resident: ResidentId,
    pub machine: Option<Written>,
    /// Counted up when the timeline is made anew: what the page holds of an
    /// earlier one is read again.
    generation: AtomicU64,
    /// Held while an event kept whole is read (`replica_reads`).
    pub bodies: tokio::sync::Semaphore,
    /// The search of the whole conversation its page runs (`replica_search`).
    pub searches: crate::replica_search::Searches,
    closed: AtomicBool,
    closing: tokio::sync::Notify,
}

pub(crate) type Shared = Arc<Reader>;

/// A page read, and where the timeline stood as it was.
pub(crate) struct Read {
    pub part: Part,
    revision: Option<u64>,
    through: Option<u64>,
}

/// The readers of the conversations a window watches, by the watch's id.
#[derive(Clone, Default)]
pub(crate) struct Readers(Arc<Mutex<HashMap<u64, Shared>>>);

impl Readers {
    pub fn insert(&self, id: u64, reader: Shared) {
        self.0.lock().expect("replica readers").insert(id, reader);
    }
    pub fn get(&self, id: u64) -> Option<Shared> {
        self.0.lock().expect("replica readers").get(&id).cloned()
    }
    pub fn close(&self, id: u64) {
        if let Some(reader) = self.0.lock().expect("replica readers").remove(&id) {
            reader.close();
        }
    }
    pub fn clear(&self) {
        for (_, reader) in self.0.lock().expect("replica readers").drain() {
            reader.close();
        }
    }
}

/// How many pages in a row with nothing this build can show are read past
/// for one part.
const UNREAD_PAGES: usize = 8;

/// Whether `error` says the machine asked is of a release that reads no
/// timeline by pages: it says so as unsupported where it knows the request,
/// and as not run, with no more said, where it does not.
fn older(error: &Error) -> bool {
    matches!(error, Error::Call(CallError::Failed(failure))
        if failure.not_run && matches!(failure.code, None | Some(fastctx_types::ipc::FailureCode::Unsupported)))
}

/// Whether a stretch of the conversation begins at `event`: a turn, the
/// user's words, or the agent's own.
fn boundary(event: &SessionEvent) -> bool {
    matches!(
        &event.happened,
        Happened::TurnStarted { .. }
            | Happened::Heard { .. }
            | Happened::Finished { item: Item { what: What::Message { .. }, .. }, .. }
    )
}

/// Whether `event` is of the work between two such beginnings, shown as one
/// group.
fn member(event: &SessionEvent) -> bool {
    match &event.happened {
        Happened::Finished { item, .. } | Happened::Began { item, .. } | Happened::Updated { item, .. } => {
            matches!(item.what, What::Tool(_) | What::Thinking { .. } | What::AgentActivity { .. })
        }
        _ => false,
    }
}

/// The event `raw` shows, read as an event: none where this release does
/// not know it.
pub(crate) fn decoded(raw: &TimelineEvent) -> Option<(SessionEvent, Option<Deferred>)> {
    match &raw.shown {
        TimelineShown::Event { event } => Some((serde_json::from_value(event.clone()).ok()?, None)),
        TimelineShown::Oversized { bytes, slim, object } => {
            let deferred = Some(Deferred { bytes: *bytes, object: object.clone() });
            let event = match slim {
                Some(slim) => serde_json::from_value(slim.clone()).ok()?,
                // Nothing of it fits a page: it stands as an item that says so.
                None => SessionEvent {
                    at: jiff::Timestamp::UNIX_EPOCH,
                    happened: Happened::Finished {
                        turn: format!("timeline:{}", raw.place.item),
                        item: Item {
                            id: raw.place.item.clone(),
                            draft: None,
                            started_at: None,
                            detail: None,
                            what: What::Other { host_kind: "fastctx:timeline-preview-unavailable".into() },
                        },
                    },
                },
            };
            Some((event, deferred))
        }
    }
}

fn shown(raw: TimelineEvent, event: &SessionEvent, deferred: Option<Deferred>) -> Result<Shown, String> {
    Ok(Shown {
        key: raw.place.item.clone(),
        place: raw.place,
        event: serde_json::value::to_raw_value(event).map_err(|error| error.to_string())?,
        deferred,
    })
}

impl Reader {
    fn new(world: World, resident: ResidentId, machine: Option<Written>) -> Shared {
        Arc::new(Self {
            world,
            resident,
            machine,
            generation: AtomicU64::new(0),
            bodies: tokio::sync::Semaphore::new(1),
            searches: Default::default(),
            closed: AtomicBool::new(false),
            closing: tokio::sync::Notify::new(),
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn close(&self) {
        self.searches.close();
        self.closed.store(true, Ordering::Release);
        self.closing.notify_waiters();
    }

    /// Resolves once the page let go of the conversation.
    pub async fn closed(&self) {
        let notified = self.closing.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.closed.load(Ordering::Acquire) {
            notified.await;
        }
    }

    /// Asks the conversation's timeline `ask`, as the node answers it.
    pub async fn ask(&self, ask: TimelineAsk) -> Result<TimelineTold, Error> {
        let reading = self.world.timeline(self.machine.clone(), ask);
        tokio::select! {
            told = tokio::time::timeout(READ_WITHIN, reading) => told.map_err(|_| Error::Other(format!("reading the timeline took over {} s", READ_WITHIN.as_secs())))?,
            () = self.closed() => Err(Error::Other("the conversation window closed".into())),
        }
    }

    /// The part of the timeline read from `from`: a page of it, begun where
    /// a stretch begins and ended where one ends when it holds such a place.
    pub async fn read(&self, from: From) -> Result<Read, Error> {
        // A page none of whose events this build reads, written by a later
        // one, is read past: the part is the first page that shows something,
        // or the last one where the timeline ends that way.
        let mut from = from;
        for _ in 0..UNREAD_PAGES {
            match self.read_page(&from).await? {
                (read, None) => return Ok(read),
                (_, Some(next)) => from = next,
            }
        }
        Ok(self.read_page(&from).await?.0)
    }

    /// One page read from `from`; and where to read on from, when none of
    /// what it holds can be shown and the timeline goes on that way.
    async fn read_page(&self, from: &From) -> Result<(Read, Option<From>), Error> {
        let (asked, cut_left, cut_right) = match from {
            From::Latest => (TimelineFrom::Latest, true, false),
            From::Before { place } => (TimelineFrom::Before { place: place.clone() }, true, false),
            From::After { place } => (TimelineFrom::After { place: place.clone() }, false, true),
        };
        let told = self
            .ask(TimelineAsk::Page {
                resident: self.resident.clone(),
                from: asked,
                max_events: PAGE_EVENTS,
                max_bytes: PAGE_BYTES,
            })
            .await?;
        Self::decode_page(self.generation(), told, cut_left, cut_right, matches!(from, From::Latest))
    }

    fn decode_page(
        generation: u64,
        told: TimelineTold,
        cut_left: bool,
        cut_right: bool,
        latest: bool,
    ) -> Result<(Read, Option<From>), Error> {
        let TimelineTold::Page { revision, initializing, events, older, newer, through } = told else {
            return Err(Error::Unexpected("the timeline did not answer a page".into()));
        };
        let ends = (events.first().map(|raw| raw.place.clone()), events.last().map(|raw| raw.place.clone()));
        let mut read: Vec<(TimelineEvent, SessionEvent, Option<Deferred>)> = events
            .into_iter()
            .filter_map(|raw| decoded(&raw).map(|(event, deferred)| (raw, event, deferred)))
            .collect();
        let (mut older, mut newer) = (older, newer);
        // Nothing of the page can be shown: read on the way it was asked,
        // past what it held.
        let onward = if read.is_empty() {
            match (cut_right, &ends) {
                (false, (Some(first), _)) if older.is_some() => Some(From::Before { place: first.clone() }),
                (true, (_, Some(last))) if newer.is_some() => Some(From::After { place: last.clone() }),
                _ => None,
            }
        } else {
            None
        };
        // More of the timeline before it: it begins where a stretch does,
        // what came before that left to the part before.
        if cut_left
            && older.is_some()
            && let Some(at) = read.iter().position(|(_, event, _)| boundary(event))
            && at > 0
        {
            read.drain(..at);
        }
        // More after it: the work a later part goes on with is left to it.
        if cut_right
            && newer.is_some()
            && let Some(last) = read.iter().rposition(|(_, event, _)| boundary(event))
            && let Some(first) = read[last + 1..].iter().position(|(_, event, _)| member(event))
            && last + 1 + first > 0
        {
            read.truncate(last + 1 + first);
        }
        // Where the next part either way is read from: just past this one.
        if older.is_some() {
            older = read.first().map(|(raw, _, _)| raw.place.clone()).or(ends.0);
        }
        if newer.is_some() {
            newer = read.last().map(|(raw, _, _)| raw.place.clone()).or(ends.1);
        }
        let mut events = Vec::with_capacity(read.len());
        for (raw, event, deferred) in read {
            events.push(shown(raw, &event, deferred).map_err(Error::Other)?);
        }
        let preparing = initializing.unwrap_or(revision.is_none());
        let stamp = revision.zip(through).filter(|_| latest).map(|(revision, through)| {
            let mut digest = Sha256::new();
            for shown in &events {
                digest.update(shown.key.as_bytes());
                digest.update(shown.place.order.as_bytes());
                digest.update(shown.event.get().as_bytes());
                if let Some(deferred) = &shown.deferred {
                    digest.update(deferred.object.as_bytes());
                }
            }
            for place in older.iter().chain(newer.iter()) {
                digest.update(place.order.as_bytes());
                digest.update(place.item.as_bytes());
            }
            format!("{revision:016x}/{through:016x}/{:x}", digest.finalize())
        });
        Ok((Read { part: Part { generation, events, older, newer, preparing, stamp }, revision, through }, onward))
    }
}

/// A display preview has no reader and cannot be used to follow a timeline.
pub(crate) async fn preview(world: &World, machine: Written, resident: ResidentId) -> Option<Part> {
    let page = world.timeline_preview(machine, resident).await?;
    let part = Reader::decode_page(0, page, true, false, true).ok()?.0.part;
    (!part.events.is_empty()).then_some(part)
}

/// A conversation opened from the timeline: its latest part, and what
/// follows it.
pub(crate) struct Opened {
    pub reader: Shared,
    pub part: Part,
    revision: Option<u64>,
    through: Option<u64>,
}

/// Opens the conversation on the latest part of its timeline; none where
/// the machine asked reads no timeline by pages, an earlier release's, which
/// is watched whole the older way.
pub(crate) async fn open(
    world: &World,
    resident: ResidentId,
    machine: Option<Written>,
) -> Result<Option<Opened>, NotWatched> {
    let reader = Reader::new(world.clone(), resident, machine);
    match reader.read(From::Latest).await {
        Ok(Read { part, revision, through }) => Ok(Some(Opened { reader, part, revision, through })),
        Err(error) if older(&error) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Follows an opened conversation until the page lets go: the records its
/// timeline gains, and what the live watch tells that is no content.
/// `again` when the page fell behind what was sent it and reads anew.
pub(crate) async fn follow(opened: &Opened, again: bool, pieces: &Sender<Piece>) {
    let reader = &opened.reader;
    let (mut revision, mut through) = (opened.revision, opened.through);
    if again {
        // What it missed cannot be told apart from what it holds: it reads
        // the latest part again, and the rest as its reader comes to it.
        match reread(reader, pieces).await {
            Some((now, at)) => (revision, through) = (now, at),
            None => return,
        }
    }
    tokio::select! {
        () = reader.closed() => {}
        () = records(reader, pieces, &mut revision, &mut through) => {}
        () = live(reader, pieces) => {}
    }
}

/// The timeline is read anew: the page is given its latest part in place of
/// all it holds. Where the timeline stands then; none once the page is gone.
async fn reread(reader: &Shared, pieces: &Sender<Piece>) -> Option<(Option<u64>, Option<u64>)> {
    let mut wait = AGAIN_FIRST;
    loop {
        reader.generation.fetch_add(1, Ordering::AcqRel);
        match reader.read(From::Latest).await {
            Ok(Read { part, revision, through }) => {
                pieces.send(Piece::Replica { part }).ok()?;
                return Some((revision, through));
            }
            Err(_) if reader.closed.load(Ordering::Acquire) => return None,
            Err(_) => {
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(AGAIN_MOST);
            }
        }
    }
}

/// Tells the page each record the timeline gains, in the order they were
/// written, for as long as the page holds the conversation.
async fn records(reader: &Shared, pieces: &Sender<Piece>, revision: &mut Option<u64>, through: &mut Option<u64>) {
    let mut wait = AGAIN_FIRST;
    let mut preparing: Option<bool> = None;
    loop {
        let moved =
            reader.ask(TimelineAsk::Moved { resident: reader.resident.clone(), after: revision.unwrap_or(0) }).await;
        let (now, initializing) = match moved {
            Ok(TimelineTold::Moved { revision, initializing }) => (revision, initializing),
            Ok(_) | Err(_) => {
                if reader.closed.load(Ordering::Acquire) {
                    return;
                }
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(AGAIN_MOST);
                continue;
            }
        };
        wait = AGAIN_FIRST;
        if let Some(initializing) = initializing
            && preparing != Some(initializing)
        {
            preparing = Some(initializing);
            if pieces.send(Piece::Preparing { preparing: initializing }).is_err() {
                return;
            }
        }
        if now == *revision {
            continue;
        }
        // A machine that tells no records, or a timeline that was not made
        // when the page opened: read anew.
        let Some(mut after) = *through else {
            match reread(reader, pieces).await {
                Some((now, at)) => (*revision, *through) = (now, at),
                None => return,
            }
            continue;
        };
        loop {
            let asked = TimelineAsk::Changes {
                resident: reader.resident.clone(),
                after,
                max_events: PAGE_EVENTS,
                max_bytes: PAGE_BYTES,
            };
            match reader.ask(asked).await {
                Ok(TimelineTold::Changes { revision: read, changes, through: next, more, .. }) => {
                    let mut told = Vec::with_capacity(changes.len());
                    for change in changes {
                        match change {
                            TimelineChange::Gone { item } => told.push(Record::Gone { key: item }),
                            TimelineChange::Is { event: raw } => {
                                if let Some((event, deferred)) = decoded(&raw)
                                    && let Ok(shown) = shown(raw, &event, deferred)
                                {
                                    told.push(Record::Is { shown });
                                }
                            }
                        }
                    }
                    if !told.is_empty()
                        && pieces.send(Piece::Records { generation: reader.generation(), records: told }).is_err()
                    {
                        return;
                    }
                    (after, *through) = (next, Some(next));
                    if !more {
                        *revision = read.or(now);
                        break;
                    }
                }
                // Made anew since: nothing the page holds is known to stand.
                Ok(TimelineTold::Reread) => {
                    match reread(reader, pieces).await {
                        Some((now, at)) => (*revision, *through) = (now, at),
                        None => return,
                    }
                    break;
                }
                Ok(_) | Err(_) => {
                    if reader.closed.load(Ordering::Acquire) {
                        return;
                    }
                    // Asked again from where the page stands.
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(AGAIN_MOST);
                    break;
                }
            }
        }
    }
}

/// Whether `event` is a request that waits for the user, or the end of one.
fn waits(event: &SessionEvent) -> bool {
    matches!(
        event.happened,
        Happened::Changed { change: SessionChange::Request { .. } | SessionChange::Resolved { .. }, .. }
    )
}

fn raw(event: &SessionEvent) -> Option<Box<RawValue>> {
    serde_json::value::to_raw_value(event).ok()
}

/// Keeps a live watch of the conversation open and tells the page what of
/// it is no content: whether it works now, the words kept on their way in,
/// and the requests that wait. A watch that ends is opened again, quietly
/// when the line holds; the page is told only when that takes long.
async fn live(reader: &Shared, pieces: &Sender<Piece>) {
    let mut again = fastctx_client::Again::default();
    let mut quiet: Option<std::time::Instant> = None;
    let mut owed: Option<fastctx_client::Interruption> = None;
    loop {
        let opening = reader.world.watch_live(reader.resident.clone(), reader.machine.clone());
        tokio::pin!(opening);
        let late = tokio::time::sleep(QUIET);
        tokio::pin!(late);
        let opened = loop {
            tokio::select! {
                opened = &mut opening => break opened,
                () = &mut late, if owed.is_some() => {
                    let interruption = owed.take().expect("guarded");
                    if pieces.send(Piece::interrupted(&interruption)).is_err() {
                        return;
                    }
                }
            }
        };
        let error = match opened {
            Ok(watching) => {
                again.came();
                owed = None;
                match told(watching, pieces).await {
                    Some(error) => error,
                    None => return,
                }
            }
            Err(error) => error,
        };
        if error.is_deleted() || error.is_refused() {
            let _ = pieces.send(Piece::failed(&error));
            return;
        }
        // A machine of an earlier release keeps no watch of this kind: the
        // page shows its copy, and says nothing of what it cannot know.
        if older(&error) {
            return std::future::pending().await;
        }
        let interruption = owed.take().unwrap_or_else(|| again.broke(error.to_string()));
        if quiet.is_none_or(|last| last.elapsed() >= QUIET_EVERY) {
            quiet = Some(std::time::Instant::now());
            owed = Some(interruption);
            continue;
        }
        if pieces.send(Piece::interrupted(&interruption)).is_err() {
            return;
        }
        again.pause(&reader.world, reader.machine.as_ref()).await;
    }
}

/// Tells the page what a live watch brings until it ends: why it ended;
/// none once the page is gone.
async fn told(mut watching: Watching, pieces: &Sender<Piece>) -> Option<Error> {
    // The requests waiting come before the watch says it is registered.
    let mut waiting: Option<Vec<Box<RawValue>>> = Some(Vec::new());
    loop {
        let next = match watching.next().await {
            Ok(Some(next)) => next,
            Ok(None) => return Some(Error::Other("the live watch ended".into())),
            Err(error) => return Some(error.into()),
        };
        let sent = match next {
            Watched::Resumed => match waiting.take() {
                Some(events) => pieces.send(Piece::Waiting { events }).and_then(|()| pieces.send(Piece::Resumed)),
                None => Ok(()),
            },
            Watched::Live { event, .. } if waits(&event) => match (&mut waiting, raw(&event)) {
                (Some(events), Some(event)) => {
                    events.push(event);
                    Ok(())
                }
                (None, Some(event)) => pieces.send(Piece::Asked { event }),
                (_, None) => Ok(()),
            },
            Watched::Now { busy, turn, kept, halted } => pieces.send(Piece::Now { busy, turn, kept, halted }),
            _ => Ok(()),
        };
        if sent.is_err() {
            return None;
        }
    }
}
