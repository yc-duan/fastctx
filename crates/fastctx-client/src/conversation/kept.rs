//! A conversation's history read through what this device keeps of it
//! (`crate::cache`): what is kept is shown at once, its latest part first,
//! and the machine the conversation lives on sends only what it lacks.

use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

use fastctx_net::history::{self, Pictures};
use fastctx_types::Address;
use fastctx_types::session::held::{Held, Segment, SegmentId};
use fastctx_types::session::{SessionEvent, Watched};

use crate::cache::{Cache, Entry, Index};
use crate::world::Error;

/// Shown first from what is kept: the latest segments, until they hold this
/// many events, about a few screens of a conversation.
const LATEST_EVENTS: usize = 200;

/// How much of the control message a watch call goes in the names of the
/// segments held may take: half of it, the rest being far more than the
/// call's envelope (names, account, ticket) ever needs. Past it the oldest
/// names are left out, and those segments come again.
const HELD_BYTES: usize = fastctx_net::wire::MAX_CONTROL_FRAME / 2;

pub(super) struct Keeping {
    cache: Arc<Cache>,
    address: Address,
    entry: Entry,
    index: Option<Index>,
    /// Segments of a history sent with its latest part first, kept until
    /// the rest comes.
    partial: HashMap<SegmentId, Vec<SessionEvent>>,
    partial_layout: Option<Vec<SegmentId>>,
    /// The layout of the history the interface was last given.
    shown: Vec<SegmentId>,
    /// Whether it was given all of it, not only its latest part.
    whole: bool,
    /// Whether the rest of what is kept is still to be given, after its
    /// latest part was.
    rest: bool,
    /// Whether the interface was given any history yet.
    given: bool,
    generation: u64,
    drawn: Vec<[u8; 32]>,
    extended: bool,
    watermark: u64,
    revision: u64,
    had_live: bool,
    current: bool,
}

impl Keeping {
    /// What is kept of the conversation at `address`: its latest part, or
    /// all of it when `whole`, when any of it is kept; and what to tell its
    /// machine this device holds.
    pub async fn open(
        cache: Arc<Cache>,
        address: Address,
        whole: bool,
    ) -> Result<(Self, Option<Vec<SessionEvent>>), Error> {
        let entry = cache.entry(&address);
        let reading = entry.clone();
        let locked = cache.clone();
        let read = blocking(move || {
            let turn = locked.turn()?;
            let generation = turn.generation();
            let Some(index) = reading.index()? else { return Ok((generation, None)) };
            // A copy its machine said was rewritten is not shown again: what
            // it keeps still saves sending what did not change.
            if index.stale {
                return Ok((generation, Some((index, None, false))));
            }
            let mut first = index.layout.len();
            let mut events = 0;
            while first > 0 && (whole || events < LATEST_EVENTS) {
                first -= 1;
                events += index.events.get(first).copied().unwrap_or_default();
            }
            while first > 0 && !index.boundaries.get(first).copied().unwrap_or(false) {
                first -= 1;
            }
            let mut latest = Vec::with_capacity(events);
            for id in &index.layout[first..] {
                latest.extend(reading.segment(id)?);
            }
            let mut index = index;
            let live = reading.live()?;
            if !live.is_empty() {
                index.version = None;
                latest.extend(live);
            }
            Ok((generation, Some((index, Some(latest), first == 0))))
        })
        .await;
        let (generation, read) = match read {
            Ok(read) => read,
            // A copy that cannot be read is no copy: it goes, and the
            // history comes whole from its machine.
            Err(error) if is_damage(&error) => {
                let removing = entry.clone();
                let locked = cache.clone();
                let generation = blocking(move || {
                    let mut turn = locked.turn()?;
                    turn.invalidate()?;
                    removing.remove()?;
                    Ok(turn.generation())
                })
                .await
                .map_err(Error::Kept)?;
                (generation, None)
            }
            Err(error) => return Err(Error::Kept(error)),
        };
        let mut keeping = Self {
            cache,
            address,
            entry,
            index: None,
            partial: HashMap::new(),
            partial_layout: None,
            shown: Vec::new(),
            whole: false,
            rest: false,
            given: false,
            generation,
            drawn: Vec::new(),
            extended: false,
            watermark: 0,
            revision: 0,
            had_live: false,
            current: false,
        };
        let Some((index, latest, whole)) = read else { return Ok((keeping, None)) };
        keeping.watermark = index.live_sequence;
        keeping.revision = index.revision;
        keeping.had_live = index.version.is_none();
        keeping.index = Some(index);
        let Some(latest) = latest else { return Ok((keeping, None)) };
        keeping.shown = keeping.index.as_ref().map(|index| index.layout.clone()).unwrap_or_default();
        keeping.whole = whole;
        keeping.rest = !whole;
        keeping.given = true;
        keeping.drawn = fingerprints(&latest);
        keeping.cache.learn(&latest);
        Ok((keeping, Some(latest)))
    }

    /// What to tell the conversation's machine this device holds.
    pub fn held(&self) -> Held {
        let Some(index) = &self.index else { return Held { version: None, segments: Vec::new(), latest_first: true } };
        let mut bytes = 0;
        let mut first = index.layout.len();
        // Each name goes as a quoted string and a comma.
        while first > 0 && bytes + index.layout[first - 1].as_str().len() + 3 <= HELD_BYTES {
            first -= 1;
            bytes += index.layout[first].as_str().len() + 3;
        }
        Held { version: index.version.clone(), segments: index.layout[first..].to_vec(), latest_first: false }
    }

    /// Whether the rest of what is kept is still to be given.
    pub fn rest_due(&self) -> bool {
        self.rest
    }

    /// The whole kept history, when only its latest part was shown.
    pub async fn rest(&mut self) -> Result<Option<Vec<SessionEvent>>, Error> {
        if !std::mem::take(&mut self.rest) {
            return Ok(None);
        }
        let Some(index) = self.index.clone() else { return Ok(None) };
        self.whole = true;
        let entry = self.entry.clone();
        let cache = self.cache.clone();
        let generation = self.generation;
        let events = blocking(move || {
            let turn = cache.turn()?;
            require_generation(turn.generation(), generation)?;
            let mut events = Vec::new();
            for id in &index.layout {
                events.extend(entry.segment(id)?);
            }
            events.extend(entry.live()?);
            Ok(events)
        })
        .await
        .map_err(Error::Kept)?;
        self.cache.learn(&events);
        self.drawn = fingerprints(&events);
        self.extended = false;
        Ok(Some(events))
    }

    /// Takes in what the conversation's machine sent of the history; returns
    /// the history to show, when it is not what was shown.
    pub async fn take(&mut self, watched: Watched) -> Result<Option<Vec<SessionEvent>>, Error> {
        self.extended = false;
        self.current = !matches!(watched, Watched::Segments { complete: false, .. });
        match watched {
            Watched::Documents { .. } => Err(Error::Unexpected("document packets are not conversation history".into())),
            // Nothing held and nothing changed: the history is empty.
            Watched::Unchanged { version } if self.index.is_none() => {
                self.keep(Some(version), Vec::new(), HashMap::new()).await
            }
            Watched::Unchanged { version } if self.had_live => {
                let layout = self.index.as_ref().expect("replayed history has an index").layout.clone();
                self.keep(Some(version), layout, HashMap::new()).await
            }
            // Held and not shown, as a copy said to be rewritten: it was
            // not, and is shown now.
            Watched::Unchanged { version } if !self.given || !self.whole => {
                let Some(index) = self.index.clone() else { return Ok(None) };
                let layout = index.layout.clone();
                self.keep(Some(version), layout, HashMap::new()).await
            }
            Watched::Unchanged { version } => {
                let Some(mut index) = self.index.clone() else { return Ok(None) };
                if index.version.as_deref() != Some(version.as_str()) {
                    index.version = Some(version.clone());
                    index.updated = jiff::Timestamp::now();
                    let entry = self.entry.clone();
                    let cache = self.cache.clone();
                    let generation = self.generation;
                    let version = version.clone();
                    blocking(move || {
                        let turn = cache.turn()?;
                        require_generation(turn.generation(), generation)?;
                        let Some(mut current) = entry.index()? else {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                "the viewed cache was cleared",
                            ));
                        };
                        if entry.live_records()?.is_empty() {
                            current.version = Some(version);
                        }
                        entry.write_index(&current)
                    })
                    .await
                    .map_err(Error::Kept)?;
                    self.index = Some(index);
                }
                Ok(None)
            }
            Watched::Segments { layout, segments, complete: false, .. } => {
                if self.partial_layout.as_ref() != Some(&layout) {
                    let keep: std::collections::HashSet<_> = layout.iter().collect();
                    self.partial.retain(|id, _| keep.contains(id));
                    self.partial_layout = Some(layout.clone());
                }
                for Segment { id, events } in segments {
                    self.partial.insert(id, events);
                }
                // The first tail gives an uncached viewer something to show.
                // Further transfer batches do not repeatedly copy and redraw
                // the growing history, or replace a viewer's existing history.
                if self.given {
                    return Ok(None);
                }
                let latest: Vec<SessionEvent> = layout
                    .iter()
                    .filter_map(|id| self.partial.get(id))
                    .flat_map(|events| events.iter().cloned())
                    .collect();
                self.shown = layout;
                self.whole = false;
                self.given = true;
                self.cache.learn(&latest);
                self.drawn = fingerprints(&latest);
                Ok(Some(latest))
            }
            Watched::Segments { version, layout, segments, complete: true } => {
                self.partial_layout = None;
                let mut fresh = std::mem::take(&mut self.partial);
                for Segment { id, events } in segments {
                    fresh.insert(id, events);
                }
                self.keep(Some(version), layout, fresh).await
            }
            // A machine that sends no segments sends the whole history as it
            // was: slimmed and cut here the same way, its images as they came,
            // since it serves none by reference.
            Watched::History { mut events } => {
                let cut = tokio::task::spawn_blocking(move || {
                    events.retain_mut(|event| !history::transient(event) && history::slim(event, &Pictures::Inline));
                    history::cut(events)
                })
                .await
                .map_err(|error| Error::Other(format!("cutting the history into segments: {error}")))?;
                let layout = cut.layout();
                let fresh = cut.segments.into_iter().map(|segment| (segment.id, segment.events)).collect();
                self.keep(None, layout, fresh).await
            }
            Watched::Live { .. } | Watched::Now { .. } => {
                Err(Error::Unexpected("a live piece where the history was due".into()))
            }
        }
    }

    async fn keep(
        &mut self,
        version: Option<String>,
        layout: Vec<SegmentId>,
        fresh: HashMap<SegmentId, Vec<SessionEvent>>,
    ) -> Result<Option<Vec<SessionEvent>>, Error> {
        let (entry, cache, address, put) =
            (self.entry.clone(), self.cache.clone(), self.address.clone(), layout.clone());
        let generation = self.generation;
        let watermark = self.watermark;
        let revision = self.revision;
        let (index, mut events, drawn, had_live) = blocking(move || {
            let turn = cache.turn()?;
            require_generation(turn.generation(), generation)?;
            let current = entry.index()?;
            if current.as_ref().is_some_and(|index| index.revision != revision) {
                return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "the conversation cache was rewritten"));
            }
            let _entry_turn = entry.turn()?;
            let mut whole = entry.put_together(&put, fresh)?;
            let retained: Vec<_> = entry
                .live_records()?
                .into_iter()
                .filter(|record| record.sequence > watermark && !represented(&record.event, &whole.events))
                .collect();
            let mut at = 0;
            let segments: Vec<Segment> = put
                .iter()
                .zip(&whole.counts)
                .map(|(id, count)| {
                    let events = whole.events[at..at + count].to_vec();
                    at += count;
                    Segment { id: id.clone(), events }
                })
                .collect();
            let boundaries = history::turn_boundaries(&segments);
            whole.events.extend(retained.iter().map(|record| record.event.clone()));
            let pictures = cache.learn(&whole.events);
            let index = Index {
                address,
                version: if retained.is_empty() { version } else { None },
                layout: put,
                events: whole.counts,
                boundaries,
                pictures,
                blobs: cache.line_keys(&whole.events),
                live_sequence: current.as_ref().map_or(0, |index| index.live_sequence),
                revision,
                rewrite: current.as_ref().and_then(|index| index.rewrite.clone()),
                updated: jiff::Timestamp::now(),
                stale: false,
            };
            entry.keep_index(&index)?;
            entry.keep_live(&retained)?;
            if let Some(previous) = &current {
                cache.retire(previous, &index)?;
            }
            let drawn = fingerprints(&whole.events);
            Ok((index, whole.events, drawn, !retained.is_empty()))
        })
        .await
        .map_err(Error::Kept)?;
        self.index = Some(index);
        self.had_live = had_live;
        let shown = self.given && self.whole && self.drawn == drawn;
        if self.given && self.whole && drawn.starts_with(&self.drawn) && !shown {
            events = events.split_off(self.drawn.len());
            self.extended = true;
        }
        self.drawn = drawn;
        self.shown = layout;
        self.whole = true;
        self.given = true;
        Ok((!shown).then_some(events))
    }

    /// Its machine said the history was rewritten (a rollback, a cut): the
    /// copy kept is not shown again before the history as it is now.
    pub fn rewritten(&mut self, event: &SessionEvent) -> Result<(), Error> {
        let turn = self.cache.turn().map_err(Error::Kept)?;
        let Some(mut index) = self.entry.index().map_err(Error::Kept)? else { return Ok(()) };
        if turn.generation() != self.generation {
            return Ok(());
        }
        let rewrite = crate::cache::key(&serde_json::to_string(event).expect("session events serialize"));
        if index.rewrite.as_deref() != Some(&rewrite) {
            index.stale = true;
            index.rewrite = Some(rewrite);
            index.revision = index
                .revision
                .checked_add(1)
                .ok_or_else(|| Error::Kept(std::io::Error::other("conversation cache revision overflow")))?;
            self.entry.write_index(&index).map_err(Error::Kept)?;
        }
        self.revision = index.revision;
        self.watermark = index.live_sequence;
        self.index = Some(index);
        self.rest = false;
        self.whole = false;
        Ok(())
    }

    /// Whether what this device kept went away while it was read, as when
    /// the cache is cleared: then the history comes whole again.
    pub fn lost(&mut self, error: &Error) -> bool {
        let lost = matches!(error, Error::Kept(error) if is_damage(error));
        if lost {
            self.index = None;
            self.partial.clear();
            self.partial_layout = None;
            self.whole = false;
            self.rest = false;
        }
        lost
    }

    pub fn extended(&self) -> bool {
        self.extended
    }

    pub fn current(&mut self) -> bool {
        std::mem::take(&mut self.current)
    }

    pub async fn record(&mut self, event: SessionEvent) -> Result<(), Error> {
        if matches!(
            event.happened,
            fastctx_types::session::Happened::Grew { .. }
                | fastctx_types::session::Happened::RealtimeGrew { .. }
                | fastctx_types::session::Happened::Changed {
                    change: fastctx_types::session::SessionChange::Request { .. }
                        | fastctx_types::session::SessionChange::Resolved { .. }
                        | fastctx_types::session::SessionChange::HistoryChanged
                        | fastctx_types::session::SessionChange::HistoryCut { .. },
                    ..
                }
        ) {
            return Ok(());
        }
        let (cache, entry, revision) = (self.cache.clone(), self.entry.clone(), self.revision);
        let saved = event.clone();
        let generation = blocking(move || {
            let turn = cache.turn()?;
            let Some(mut index) = entry.index()? else { return Ok(None) };
            if index.revision != revision || index.stale {
                return Ok(None);
            }
            index.live_sequence = index
                .live_sequence
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("conversation live sequence overflow"))?;
            let blobs = cache.line_keys(std::slice::from_ref(&saved));
            let pictures = cache.learn(std::slice::from_ref(&saved));
            let mut listed = false;
            for picture in pictures {
                if !index.pictures.contains(&picture) {
                    index.pictures.push(picture);
                    listed = true;
                }
            }
            for blob in blobs {
                if !index.blobs.contains(&blob) {
                    index.blobs.push(blob);
                    listed = true;
                }
            }
            // The index is written again only when what it lists grew: the
            // sequence alone goes in its own small file.
            if listed {
                entry.write_index(&index)?;
            }
            entry.set_live_sequence(index.live_sequence)?;
            entry.record(&saved, index.live_sequence)?;
            Ok(Some(turn.generation()))
        })
        .await
        .map_err(Error::Kept)?;
        if let Some(generation) = generation {
            self.generation = generation;
        }
        self.drawn.push(Sha256::digest(serde_json::to_vec(&event).expect("session events serialize")).into());
        Ok(())
    }

    pub fn renew_generation(&mut self) -> Result<(), Error> {
        let turn = self.cache.turn().map_err(Error::Kept)?;
        self.generation = turn.generation();
        self.index = self.entry.index().map_err(Error::Kept)?;
        self.watermark = self.index.as_ref().map_or(0, |index| index.live_sequence);
        self.revision = self.index.as_ref().map_or(0, |index| index.revision);
        self.had_live = !self.entry.live_records().map_err(Error::Kept)?.is_empty();
        Ok(())
    }

    pub fn discard_damage(&mut self, error: &Error) -> Result<(), Error> {
        if matches!(error, Error::Kept(error) if error.kind() == std::io::ErrorKind::InvalidData) {
            let mut turn = self.cache.turn().map_err(Error::Kept)?;
            turn.invalidate().map_err(Error::Kept)?;
            self.entry.remove().map_err(Error::Kept)?;
        }
        Ok(())
    }
}

fn represented(event: &SessionEvent, history: &[SessionEvent]) -> bool {
    use fastctx_types::session::Happened;
    history.iter().any(|kept| match (&event.happened, &kept.happened) {
        (Happened::Began { turn, item }, Happened::Finished { turn: at, item: finished }) => {
            turn == at && item.id == finished.id
        }
        (Happened::TurnStarted { turn }, Happened::TurnStarted { turn: at } | Happened::TurnEnded { turn: at, .. }) => {
            turn == at
        }
        (Happened::Heard { delivery: Some(delivery), .. }, Happened::Heard { delivery: Some(at), .. }) => {
            delivery == at
        }
        (Happened::Heard { message: Some(message), .. }, Happened::Heard { message: Some(at), .. }) => {
            message.id == at.id
        }
        _ => event == kept,
    })
}

fn fingerprints(events: &[SessionEvent]) -> Vec<[u8; 32]> {
    events
        .iter()
        .map(|event| {
            let mut rendered = event.clone();
            if matches!(rendered.happened, fastctx_types::session::Happened::Titled { .. }) {
                rendered.at = jiff::Timestamp::UNIX_EPOCH;
            }
            Sha256::digest(serde_json::to_vec(&rendered).expect("session events serialize")).into()
        })
        .collect()
}

fn require_generation(actual: u64, expected: u64) -> std::io::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "the viewed cache was cleared"))
    }
}

/// A kept file that is gone or damaged.
fn is_damage(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData)
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> std::io::Result<T> {
    tokio::task::spawn_blocking(work).await.map_err(std::io::Error::other)?
}
