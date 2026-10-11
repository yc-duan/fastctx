//! A conversation's history read through what this device keeps of it
//! (`crate::cache`): what is kept is shown at once, its latest part first,
//! and the machine the conversation lives on sends only what it lacks.

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
    /// The layout of the history the interface was last given.
    shown: Vec<SegmentId>,
    /// Whether it was given all of it, not only its latest part.
    whole: bool,
    /// Whether the rest of what is kept is still to be given, after its
    /// latest part was.
    rest: bool,
    /// Whether the interface was given any history yet.
    given: bool,
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
        let read = blocking(move || {
            let Some(index) = reading.index()? else { return Ok(None) };
            // A copy its machine said was rewritten is not shown again: what
            // it keeps still saves sending what did not change.
            if index.stale {
                return Ok(Some((index, None, false)));
            }
            let mut first = index.layout.len();
            let mut events = 0;
            while first > 0 && (whole || events < LATEST_EVENTS) {
                first -= 1;
                events += index.events.get(first).copied().unwrap_or_default();
            }
            let mut latest = Vec::with_capacity(events);
            for id in &index.layout[first..] {
                latest.extend(reading.segment(id)?);
            }
            Ok(Some((index, Some(latest), first == 0)))
        })
        .await;
        let read = match read {
            Ok(read) => read,
            // A copy that cannot be read is no copy: it goes, and the
            // history comes whole from its machine.
            Err(error) if is_damage(&error) => {
                let removing = entry.clone();
                blocking(move || removing.remove()).await.map_err(Error::Kept)?;
                None
            }
            Err(error) => return Err(Error::Kept(error)),
        };
        let mut keeping = Self {
            cache,
            address,
            entry,
            index: None,
            partial: HashMap::new(),
            shown: Vec::new(),
            whole: false,
            rest: false,
            given: false,
        };
        let Some((index, latest, whole)) = read else { return Ok((keeping, None)) };
        keeping.index = Some(index);
        let Some(latest) = latest else { return Ok((keeping, None)) };
        keeping.shown = keeping.index.as_ref().map(|index| index.layout.clone()).unwrap_or_default();
        keeping.whole = whole;
        keeping.rest = !whole;
        keeping.given = true;
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
        let events = blocking(move || {
            let mut events = Vec::new();
            for id in &index.layout {
                events.extend(entry.segment(id)?);
            }
            Ok(events)
        })
        .await
        .map_err(Error::Kept)?;
        self.cache.learn(&events);
        Ok(Some(events))
    }

    /// Takes in what the conversation's machine sent of the history; returns
    /// the history to show, when it is not what was shown.
    pub async fn take(&mut self, watched: Watched) -> Result<Option<Vec<SessionEvent>>, Error> {
        match watched {
            // Nothing held and nothing changed: the history is empty.
            Watched::Unchanged { version } if self.index.is_none() => {
                self.keep(Some(version), Vec::new(), HashMap::new()).await
            }
            // Held and not shown, as a copy said to be rewritten: it was
            // not, and is shown now.
            Watched::Unchanged { version } if !self.given => {
                let Some(index) = self.index.clone() else { return Ok(None) };
                let layout = index.layout.clone();
                self.keep(Some(version), layout, HashMap::new()).await
            }
            Watched::Unchanged { version } => {
                let Some(mut index) = self.index.clone() else { return Ok(None) };
                if index.version.as_deref() != Some(version.as_str()) {
                    index.version = Some(version);
                    index.updated = jiff::Timestamp::now();
                    let entry = self.entry.clone();
                    let kept = index.clone();
                    blocking(move || {
                        let _turn = entry.turn()?;
                        entry.keep_index(&kept)
                    })
                    .await
                    .map_err(Error::Kept)?;
                    self.index = Some(index);
                }
                Ok(None)
            }
            Watched::Segments { layout, segments, complete: false, .. } => {
                for Segment { id, events } in segments {
                    self.partial.insert(id, events);
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
                Ok(Some(latest))
            }
            Watched::Segments { version, layout, segments, complete: true } => {
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
                    events.retain_mut(|event| history::slim(event, &Pictures::Inline));
                    history::cut(events)
                })
                .await
                .map_err(|error| Error::Other(format!("cutting the history into segments: {error}")))?;
                let layout = cut.layout();
                let fresh = cut.segments.into_iter().map(|segment| (segment.id, segment.events)).collect();
                self.keep(None, layout, fresh).await
            }
            Watched::Live { .. } => Err(Error::Unexpected("a live event where the history was due".into())),
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
        let (index, events) = blocking(move || {
            let _turn = entry.turn()?;
            let whole = entry.put_together(&put, fresh)?;
            let pictures = cache.learn(&whole.events);
            let index = Index {
                address,
                version,
                layout: put,
                events: whole.counts,
                pictures,
                updated: jiff::Timestamp::now(),
                stale: false,
            };
            entry.keep_index(&index)?;
            Ok((index, whole.events))
        })
        .await
        .map_err(Error::Kept)?;
        self.index = Some(index);
        let shown = self.given && self.whole && self.shown == layout;
        self.shown = layout;
        self.whole = true;
        self.given = true;
        Ok((!shown).then_some(events))
    }

    /// Its machine said the history was rewritten (a rollback, a cut): the
    /// copy kept is not shown again before the history as it is now.
    pub fn rewritten(&self) -> Result<(), Error> {
        let Some(mut index) = self.index.clone() else { return Ok(()) };
        index.stale = true;
        let _turn = self.entry.turn().map_err(Error::Kept)?;
        self.entry.keep_index(&index).map_err(Error::Kept)
    }

    /// Whether what this device kept went away while it was read, as when
    /// the cache is cleared: then the history comes whole again.
    pub fn lost(&mut self, error: &Error) -> bool {
        let lost = matches!(error, Error::Kept(error) if is_damage(error));
        if lost {
            self.index = None;
            self.partial.clear();
        }
        lost
    }
}

/// A kept file that is gone or damaged.
fn is_damage(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData)
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> std::io::Result<T> {
    tokio::task::spawn_blocking(work).await.map_err(std::io::Error::other)?
}
