//! A conversation's history as a viewer reads it, from its timeline in the
//! asset library rather than from its sources whole (design §28.52 52-1):
//! the timeline is brought to the sources as they stand — read after the
//! viewer began to hear the conversation's events, so it holds every one
//! before them, as a reading of the sources begun then would — and read a
//! page at a time, each event whole (one too large for a record from its
//! object), never the history whole in memory. Where the asset library does
//! not read the conversation's timeline here (it places it nowhere, its
//! timeline not made yet), or an object of it is not here, the viewer reads
//! the sources as before (`talk`).

use super::local::{self, Held};
use super::reads::{Anchor, PAGE_BYTES, PAGE_ITEMS, Place};
use crate::Node;
use fastctx_client::timeline_copy::Shown;
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::session::SessionEvent;
use std::sync::Arc;

/// How many times a reading begins again, the timeline written as it was
/// read; the last with the timeline's build held, so that only a device
/// ordering it elsewhere writes it meanwhile. Written still, the viewer is
/// told to ask again — its sources are not read whole for it
/// (codex-e7b1552a review-s19 P1-3).
pub(crate) const TRIES: usize = 3;

/// An event kept as an object larger than this is not read whole for a
/// viewer: what is shown in its place stands for it, whatever its length
/// (codex-e7b1552a review-s19 P1-2) — the viewer holds a slimmed history.
const WHOLE_MOST: u64 = 32 << 20;

/// What a reading hands on at once at most, as its events are made (one
/// event larger than it alone): a page of events that stay long once
/// slimmed is handed on a part at a time (codex-e7b1552a review-s21 P1-2).
const BATCH_BYTES: usize = 4 << 20;

/// The timeline's build held while a reading's last try reads it: nothing
/// of this machine writes it meanwhile.
pub(crate) struct Steady(#[allow(dead_code)] super::serial::Held);

pub(crate) async fn steady(resident: &ResidentId) -> Steady {
    Steady(super::tail::PRODUCING.hold(resident).await)
}

/// What a reading says once it began again `TRIES` times, the timeline
/// written each time.
pub(crate) fn kept_changing(resident: &ResidentId) -> Failure {
    Failure::new(format!("{resident}'s timeline kept changing as it was read: open it again"))
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// The timelines brought for viewers at once, besides those the timeline's
/// job brings (codex-e7b1552a P1-6): many opened together, as after an
/// upgrade, wait their turn rather than each make one.
static BRINGING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// The conversation's timeline brought to its sources as they stand, and
/// where it is read here, with whether a source could not be read (the
/// timeline then holds what it held of it): none where it is not read here
/// as the sources stand — not placed, another device ordering it as this
/// one goes on (codex-e7b1552a P1-4) — for the caller to read the sources.
pub(crate) async fn brought(node: &Arc<Node>, resident: &ResidentId) -> Result<Option<(Held, bool)>, Failure> {
    // Made by a task of its own, which keeps its turn to the end: a viewer
    // gone meanwhile gives none back before the making is done
    // (codex-e7b1552a review-s19 P2-1).
    let turn = BRINGING.acquire().await.map_err(failure)?;
    let (making, whose) = (node.clone(), resident.clone());
    let produced = fastctx_platform::task::spawn(async move {
        let _turn = turn;
        let produced = super::tail::produce(&making, &whose, false).await;
        // What the timeline's own job does after it, done by the task too:
        // a viewer gone meanwhile does not take it away (review-s21 P2-2).
        if produced.as_ref().is_ok_and(|produced| produced.long_changed) {
            super::tail::check_wanted(&making, &whose);
        }
        produced
    })
    .await
    .map_err(failure)?;
    let produced = match produced {
        Ok(produced) => produced,
        Err(error) => {
            node.log.line(format_args!("{resident}'s timeline could not be brought for a viewer ({}): its sources are read", error.message));
            return Ok(None);
        }
    };
    if produced.handing {
        return Ok(None);
    }
    let held = fastctx_platform::roles::in_place("reading a timeline", || local::held(node, resident))?;
    Ok(held.map(|held| (held, produced.unread)))
}

/// Where the conversation's timeline read here stands now, as a version of
/// what it holds: none where it is not read here.
pub(crate) fn standing(node: &Node, resident: &ResidentId) -> Result<Option<u64>, Failure> {
    let Some(held) = fastctx_platform::roles::in_place("reading a timeline", || local::held(node, resident))? else { return Ok(None) };
    let timeline = <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM;
    held.library().view_revision(held.scope(), timeline).map_err(failure)
}

/// What the next page of a reading holds.
pub(crate) enum Next<T> {
    Page(Vec<T>),
    /// Every page read.
    End,
    /// The timeline was written since the reading began: it begins again.
    Changed,
    /// An event's object is not here: the sources are read instead.
    Missing,
}

/// A reading of the timeline a page at a time, from its first event on or
/// from its last back.
pub(crate) struct Reader {
    held: Held,
    forward: bool,
    anchor: Anchor,
    revision: Option<u64>,
    done: bool,
    /// What of the page read is not handed on yet (`next_as`).
    pending: std::collections::VecDeque<Shown>,
    /// The conversation whose runs' facts each event read is given as they
    /// stand now (`run_facts::attach`): an item settled long since tells of
    /// a run that went on after it (codex-e7b1552a P1-3).
    runs: Option<fastctx_types::layout::ConversationDir>,
}

impl Reader {
    pub(crate) fn forward(held: Held) -> Self {
        Self { held, forward: true, anchor: Self::start(true), revision: None, done: false, pending: Default::default(), runs: None }
    }

    pub(crate) fn backward(held: Held) -> Self {
        Self { held, forward: false, anchor: Self::start(false), revision: None, done: false, pending: Default::default(), runs: None }
    }

    /// Each event read given its runs' facts as `directory` holds them now.
    pub(crate) fn with_runs(mut self, directory: fastctx_types::layout::ConversationDir) -> Self {
        self.runs = Some(directory);
        self
    }

    /// Where the timeline stood as it was read.
    pub(crate) fn revision(&self) -> Option<u64> {
        self.revision
    }

    fn start(forward: bool) -> Anchor {
        if forward { Anchor::After(Place { order: Vec::new(), item: Vec::new() }) } else { Anchor::Latest }
    }

    /// Reads from the start again.
    pub(crate) fn restart(&mut self) {
        self.anchor = Self::start(self.forward);
        self.revision = None;
        self.done = false;
        self.pending.clear();
    }

    /// The next page's events as the timeline shows them, in the reading's
    /// order (the latest first, read back). Call on a thread that may block.
    pub(crate) fn next_shown(&mut self) -> Result<Next<Shown>, Failure> {
        if self.done {
            return Ok(Next::End);
        }
        let page = local::page(&self.held, &self.anchor, PAGE_ITEMS, PAGE_BYTES)?;
        if self.revision.is_some() && page.revision != self.revision {
            return Ok(Next::Changed);
        }
        self.revision = page.revision;
        let next = if self.forward { page.newer.map(Anchor::After) } else { page.older.map(Anchor::Before) };
        match next {
            Some(anchor) => self.anchor = anchor,
            None => self.done = true,
        }
        let mut shown: Vec<Shown> = page.entries.into_iter().map(|entry| entry.shown).collect();
        if !self.forward {
            shown.reverse();
        }
        Ok(Next::Page(shown))
    }

    /// The next page's events, whole. Call on a thread that may block.
    pub(crate) fn next(&mut self) -> Result<Next<SessionEvent>, Failure> {
        self.next_as(&mut Some)
    }

    /// The next page's events, each whole as it is read — given its runs'
    /// facts, then made what `make` makes of it before the next is read, so
    /// that a page of events kept as objects is never whole in memory at
    /// once (codex-e7b1552a review-s19 P1-2) — those it keeps. One kept as
    /// an object past `WHOLE_MOST` is what is shown in its place, when
    /// something is. Call on a thread that may block.
    pub(crate) fn next_as(&mut self, make: &mut dyn FnMut(SessionEvent) -> Option<SessionEvent>) -> Result<Next<SessionEvent>, Failure> {
        if self.pending.is_empty() {
            match self.next_shown()? {
                Next::Page(shown) => self.pending = shown.into(),
                Next::End => return Ok(Next::End),
                Next::Changed => return Ok(Next::Changed),
                Next::Missing => return Ok(Next::Missing),
            }
        }
        // As much of the page as makes `BATCH_BYTES` once made.
        let (mut events, mut bytes) = (Vec::new(), 0);
        while bytes < BATCH_BYTES
            && let Some(shown) = self.pending.pop_front()
        {
            let read = match &shown {
                Shown::Oversized { object, slim: Some(slim), .. } if object.size > WHOLE_MOST => {
                    Some(serde_json::from_slice::<SessionEvent>(slim).map_err(failure)?)
                }
                _ => self.whole(&shown)?,
            };
            let Some(mut event) = read else {
                // Not here: gone with a writing since the page was read, or
                // never here.
                return Ok(if self.moved()? { Next::Changed } else { Next::Missing });
            };
            if let Some(directory) = &self.runs {
                crate::run_facts::attach(directory, &mut event).map_err(failure)?;
            }
            if let Some(made) = make(event) {
                bytes += fastctx_net::watch_queue::size(&made);
                events.push(made);
            }
        }
        Ok(Next::Page(events))
    }

    /// Whether the timeline was written since the reading began.
    pub(crate) fn moved(&self) -> Result<bool, Failure> {
        let timeline = <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM;
        Ok(self.held.library().view_revision(self.held.scope(), timeline).map_err(failure)? != self.revision)
    }

    /// The event `shown` stands for, whole: none where its object is not
    /// here.
    pub(crate) fn whole(&self, shown: &Shown) -> Result<Option<SessionEvent>, Failure> {
        match shown {
            Shown::Event(encoded) => Ok(Some(serde_json::from_slice(encoded).map_err(failure)?)),
            Shown::Oversized { object, .. } => {
                let length = u32::try_from(object.size).map_err(|_| failure("an event's object is past what one read takes"))?;
                let Some(bytes) = local::object(&self.held, &object.hash, 0, length)? else { return Ok(None) };
                Ok(Some(serde_json::from_slice(&bytes).map_err(failure)?))
            }
        }
    }
}

/// The page `next` reads, with the reader handed back, on a thread that may
/// block.
pub(crate) async fn next_page(mut reader: Reader) -> Result<(Reader, Next<SessionEvent>), Failure> {
    fastctx_platform::task::spawn_blocking(move || {
        let next = reader.next();
        next.map(|next| (reader, next))
    })
    .await
    .map_err(failure)?
}

/// The whole history of `resident` as the timeline holds it, in order: none
/// where it is not read here. What a viewer that holds nothing of it and
/// asks for it whole is sent (an older client's watch, §28.52 52-4): the
/// answer is the whole history, so it is held, but no source is read whole
/// beside it.
pub(crate) async fn history(node: &Arc<Node>, resident: &ResidentId) -> Result<Option<Vec<SessionEvent>>, Failure> {
    let Some((held, _)) = brought(node, resident).await? else { return Ok(None) };
    let mut reader = Reader::forward(held).with_runs(node.world.conversation(resident));
    let mut history = Vec::new();
    let (mut tries, mut _steady) = (0, None);
    loop {
        let (back, next) = next_page(reader).await?;
        reader = back;
        match next {
            Next::Page(page) => history.extend(page),
            Next::End => return Ok(Some(history)),
            Next::Changed => {
                tries += 1;
                if tries >= TRIES {
                    return Err(kept_changing(resident));
                }
                if tries + 1 == TRIES {
                    _steady = Some(self::steady(resident).await);
                }
                history.clear();
                reader.restart();
            }
            Next::Missing => return Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::assets::tests::{Choices, Device, library, note, words};
    use crate::timeline::assets::{Span, write};

    fn encoded(events: &[SessionEvent]) -> Vec<String> {
        events.iter().map(|event| serde_json::to_string(event).unwrap()).collect()
    }

    /// Every event read off the timeline, from its start or back from its
    /// end.
    fn read(held: Held, forward: bool) -> Vec<SessionEvent> {
        let mut reader = if forward { Reader::forward(held) } else { Reader::backward(held) };
        let mut events = Vec::new();
        loop {
            match reader.next().unwrap() {
                Next::Page(page) => events.extend(page),
                Next::End => break,
                Next::Changed | Next::Missing => panic!("read as it stands"),
            }
        }
        if !forward {
            events.reverse();
        }
        events
    }

    /// The timeline read a page at a time, either way, is the history written
    /// into it, event for event: alike events, named twice or at one moment,
    /// those kept whole as objects; and one written into it as it is read
    /// has the reading begin again.
    #[test]
    fn the_timeline_read_a_page_at_a_time_is_the_history_written() {
        let dir = std::env::temp_dir().join(format!("fastctx-timeline-viewed-{}", std::process::id()));
        let (store, placed) = library(&dir);
        let signer: Arc<dyn fastctx_content::v2::Signer> = Arc::new(Device);
        let resident: ResidentId = "claude-a0a0".parse().unwrap();
        let mut choices = Choices(0x9e37_79b9_7f4a_7c15);
        for round in 0..4 {
            let count = 300 + choices.pick(700) as usize;
            let history: Vec<SessionEvent> = (0..count)
                .map(|index| {
                    let seconds = (index / 3) as i64;
                    match choices.pick(10) {
                        0 => note(["a", "b"][choices.pick(2) as usize], seconds),
                        1 => words(&format!("m{}", choices.pick(40)), &"long ".repeat(20_000), seconds),
                        _ => words(&format!("m{round}-{index}"), &format!("w{}", choices.pick(9)), seconds),
                    }
                })
                .collect();
            write(&store, signer.clone(), &resident, &placed, &history, Span::Whole, &Default::default(), None).unwrap();
            let held = || Held::at(store.clone(), "s".into());
            assert_eq!(encoded(&read(held(), true)), encoded(&history), "round {round}, read on");
            assert_eq!(encoded(&read(held(), false)), encoded(&history), "round {round}, read back");
        }
        // Written as it is read: the next page says so, and from the start
        // it is the history now.
        let history: Vec<SessionEvent> = (0..900).map(|index| words(&format!("n{index}"), "w", index as i64)).collect();
        write(&store, signer.clone(), &resident, &placed, &history, Span::Whole, &Default::default(), None).unwrap();
        let mut reader = Reader::forward(Held::at(store.clone(), "s".into()));
        assert!(matches!(reader.next().unwrap(), Next::Page(page) if page.len() == PAGE_ITEMS as usize));
        let mut grown = history.clone();
        grown.push(words("n-last", "w", 2_000));
        write(&store, signer, &resident, &placed, &grown, Span::Whole, &Default::default(), None).unwrap();
        assert!(matches!(reader.next().unwrap(), Next::Changed));
        reader.restart();
        let mut again = Vec::new();
        loop {
            match reader.next().unwrap() {
                Next::Page(page) => again.extend(page),
                Next::End => break,
                _ => panic!("read as it stands"),
            }
        }
        assert_eq!(encoded(&again), encoded(&grown));
        drop(store);
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    }
}
