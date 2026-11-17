//! A Claude resident's sessions read whole into a reading's work store
//! rather than held in memory (design §28.51 ⑤-4): each session's record
//! scanned into the store and projected from it
//! (`transcript::read_history_kept`), its events kept there as they are
//! made; the few events the World adds — notes of what could not be read,
//! each session's title and note of its problems — held beside them; all
//! placed in the World's turns (`placement`) by what each shows of itself,
//! then each shown as a history shows it, keyed for merging with the
//! World's record, and kept again in the order placed
//! (`Spill::push_hosted`). The same steps, in the same order, as
//! `super::hosted` and `talk::assemble` take with all held in memory.

use super::placement::{self, Spot, Spots, Turns};
use crate::talk::merging;
use crate::timeline::Early;
use crate::timeline::spill::{Lines, Spill, pack_event};
use fastctx_hosts::claude::transcript::follow::{self, Restart, Seam, TailStore};
use fastctx_hosts::claude::transcript::{self, HistoryCut, Transcript};
use fastctx_types::layout::ConversationDir;
use fastctx_types::session::attachments::Receipts;
use fastctx_types::session::{Happened, SessionEvent};
use jiff::Timestamp;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;

/// Where an event of the history is (`Reading::refs`): kept in the store,
/// by its number there, or, with this bit, held among the few the World
/// adds, by its number among them.
const HELD: u32 = 1 << 31;

/// The sessions of a resident read so far, their events kept on disk.
pub(crate) struct Reading {
    spill: RefCell<Spill>,
    /// What placing looks at of each event (`Spots`), and where the event
    /// is (`HELD`), in the order of the sessions' history.
    spots: Spots,
    refs: Vec<u32>,
    /// The events the World adds: notes of what could not be read, titles.
    held: Vec<SessionEvent>,
    /// Where the session being read begins in `spots` and `refs` — its
    /// title's place, held for it while it is read — and how many events
    /// the store kept then.
    start: usize,
    mark: u32,
    /// How many sessions stopped short (§28.49): what each of their events
    /// is known by (`super::freeze`) is kept in the store, a set for each
    /// (`Spill::push_known`).
    stopped: u32,
    /// The latest time of an event of the history so far, and of one the
    /// attempt under way made.
    latest: Option<Timestamp>,
    trying: Option<Timestamp>,
}

/// The history a `Reading` placed: kept in the store in its order
/// (`Spill::hosted`), with the sets of what sessions stopped short are
/// known by; each event's key for merging with the World's record.
pub(crate) struct Placed {
    pub(crate) spill: Spill,
    /// Each event's key for merging and its time (`merging::HostKeys`),
    /// numbered by names closed over the World's record.
    pub(crate) keys: merging::HostKeys,
    pub(crate) names: merging::Names,
    /// The events as placed that `place` was asked to tell apart.
    pub(crate) told: Vec<SessionEvent>,
}

/// What placing a reading asks of the World's record (`Reading::place`):
/// its turns (all it records), and of the events merged with the history
/// (all, or those of a window) the turns words were heard in and the names
/// they name, closed over them — of a record held, or of one kept on disk
/// (`timeline::world`, its keys kept there as these names number them).
pub(crate) enum Placing<'a> {
    /// Held: as the tests and the bench weigh the store against.
    #[cfg_attr(not(any(test, feature = "bench")), allow(dead_code))]
    Held { recorded: &'a [SessionEvent], merged: &'a [SessionEvent] },
    Kept { turns: Turns<'a>, sent: HashSet<String>, names: merging::Names },
}

/// The receipts of the conversation in `.0`, read afresh for each delivery
/// asked of: none are kept as a reading goes (design §28.51).
pub(crate) struct Fresh(pub(crate) ConversationDir);

impl Receipts for Fresh {
    fn of(&self, delivery: &fastctx_types::NoticeId) -> Vec<fastctx_types::attachments::Attachment> {
        crate::attachments::delivered::at(self.0.clone()).of(delivery)
    }
}

impl Reading {
    /// A reading kept in `spill`, begun anew.
    pub(crate) fn new(mut spill: Spill) -> Result<Self, String> {
        spill.begin()?;
        Ok(Reading {
            spill: RefCell::new(spill),
            spots: Spots::default(),
            refs: Vec::new(),
            held: Vec::new(),
            start: 0,
            mark: 0,
            stopped: 0,
            latest: None,
            trying: None,
        })
    }

    /// The latest time of an event the sessions accepted and the notes held
    /// so far show.
    pub(crate) fn latest(&self) -> Option<Timestamp> {
        self.latest
    }

    /// The 8-byte digest (`digest`) of every record id the sessions
    /// accepted so far hold (`Spill::accept_earlier`), kept with those a
    /// seam moved past (`Spill::push_moved`).
    pub(crate) fn keep_earlier_moved(&self, digest: &dyn Fn(&str) -> u64) -> Result<(), String> {
        let mut after: Option<String> = None;
        loop {
            let page = self.spill.borrow().earlier_from(after.as_deref())?;
            let Some(last) = page.last().cloned() else { break };
            let mut spill = self.spill.borrow_mut();
            for uuid in &page {
                spill.push_moved(digest(uuid))?;
            }
            after = Some(last);
        }
        Ok(())
    }

    /// The digests kept of the record ids a seam moved past, rising.
    #[cfg(test)]
    pub(crate) fn moved(&self) -> Result<Vec<u64>, String> {
        let bytes = self.spill.borrow_mut().moved_bytes()?;
        Ok(bytes.chunks_exact(8).map(|chunk| u64::from_le_bytes(chunk.try_into().expect("eight bytes"))).collect())
    }

    /// Reads the record at `path` once — an attempt at the session being
    /// read — its rows kept in the store, its events kept there as they
    /// are made, each first through `images`; what an attempt before took
    /// is let go. A row a session read before holds (`accept`) gives no
    /// events (`transcript::read_continuing`).
    pub(crate) fn read(&mut self, path: &Path, cuts: &[HistoryCut], images: &mut dyn FnMut(&mut SessionEvent)) -> std::io::Result<Transcript> {
        self.begin_attempt()?;
        let (spill, spots, refs, trying) = (&self.spill, &mut self.spots, &mut self.refs, &mut self.trying);
        let earlier = |uuid: &str| spill.borrow().is_earlier(uuid).map_err(std::io::Error::other);
        transcript::read_history_kept(path, cuts, &earlier, &mut Lines(spill), &mut |event| keep(spill, spots, refs, trying, images, event))
    }

    /// Reads the record at `path` past `seam` once, as a follower does
    /// (`follow::tail_kept`, design §28.51 ⑤-5) — an attempt at the session
    /// being read, as `read` is: its rows kept in the store, its events kept
    /// there as they are made, each first through `images`; `accept` and
    /// `ended` as `follow::tail` takes them, `seen` the record ids before
    /// the seam. The record id of each row the seam moved past is kept in
    /// the store by its `digest` (`Spill::push_moved`, not held); of
    /// `asked`, those the record holds are in the transcript's `uuids`. A
    /// row a session accepted before holds gives no events.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tail(
        &mut self,
        path: &Path,
        seam: &Seam,
        accept: &dyn Fn(Timestamp) -> bool,
        ended: bool,
        seen: follow::Seen<'_>,
        asked: &[String],
        digest: &dyn Fn(&str) -> u64,
        images: &mut dyn FnMut(&mut SessionEvent),
    ) -> std::io::Result<Result<follow::Tail, Restart>> {
        self.begin_attempt()?;
        let (spill, spots, refs, trying) = (&self.spill, &mut self.spots, &mut self.refs, &mut self.trying);
        let earlier = |uuid: &str| spill.borrow().is_earlier(uuid).map_err(std::io::Error::other);
        let mut lines = Lines(spill);
        let mut events = |event| keep(spill, spots, refs, trying, images, event);
        let mut moved_past = |uuid: &str| spill.borrow_mut().push_moved(digest(uuid)).map_err(std::io::Error::other);
        follow::tail_kept(path, seam, &earlier, accept, ended, seen, TailStore { rows: &mut lines, events: &mut events, moved_past: &mut moved_past, asked })
    }

    /// Lets go what an attempt before took, and holds the place of the
    /// title before the session's events until it is known whether there
    /// is one (`accept`).
    fn begin_attempt(&mut self) -> std::io::Result<()> {
        self.again().map_err(std::io::Error::other)?;
        self.spots.push(Spot { at: Timestamp::MIN, hidden: false, heard: false });
        self.refs.push(HELD);
        Ok(())
    }

    /// Lets go what the attempt under way took.
    fn again(&mut self) -> Result<(), String> {
        self.trying = None;
        self.spots.truncate(self.start);
        self.refs.truncate(self.start);
        let mut spill = self.spill.borrow_mut();
        spill.truncate_raw(self.mark)?;
        spill.clear_moved()?;
        spill.reading()
    }

    /// Whether `read`, the record read last, holds `uuid`: among its rows,
    /// or the queued messages' sources it names.
    pub(crate) fn holds(&self, read: &Transcript, uuid: &str) -> Result<bool, String> {
        if read.uuids.contains(uuid) {
            return Ok(true);
        }
        let mut spill = self.spill.borrow_mut();
        Ok(spill.line_of(uuid)?.is_some() || spill.is_member(transcript::SET_QUEUED, uuid)?)
    }

    /// `read`, the record read last, is the session `id`'s, begun at
    /// `began`: its UUIDs join those the sessions after it see as earlier;
    /// its title comes first, then its events, then a note of its
    /// problems, as `super::session_events` makes them; stopped short
    /// (§28.49), what each but the note is known by is kept
    /// (`super::freeze`).
    pub(crate) fn accept(&mut self, id: &str, began: Timestamp, read: &Transcript) -> Result<(), String> {
        self.spill.borrow_mut().accept_earlier(read.uuids.iter().map(String::as_str))?;
        let title = read.title.clone().map(|title| SessionEvent { at: began, happened: Happened::Titled { title } });
        self.latest = self.latest.max(self.trying.take());
        // The title in the place held for it, or none there.
        match &title {
            Some(title) => {
                self.latest = self.latest.max(Some(title.at));
                self.spots.set(self.start, placement::spot(title));
                self.refs[self.start] = HELD | self.held.len() as u32;
                self.held.push(title.clone());
            }
            None => {
                self.spots.remove(self.start);
                self.refs.remove(self.start);
            }
        }
        let problem = super::problem_note(id, &read.problems, read.problems_omitted);
        if let Some(text) = &problem {
            self.hold(super::note(began, text.clone()));
        }
        if read.refused.is_some() {
            let record = self.stopped;
            self.stopped += 1;
            // Kept as they are known, a batch of the session's events at a
            // time, as the store keeps them: none held beyond its batch.
            let mut known = HashSet::new();
            if let Some(title) = &title {
                super::freeze(&mut known, std::slice::from_ref(title));
            }
            let (mut from, end) = (self.mark, self.spill.borrow().raws());
            loop {
                let mut spill = self.spill.borrow_mut();
                for key in known.drain() {
                    spill.push_known(record, key)?;
                }
                if from >= end {
                    break;
                }
                let batch = spill.raws_from(from, end)?;
                let Some(last) = batch.last().map(|(number, _)| *number) else { break };
                for (_, event) in &batch {
                    super::freeze(&mut known, std::slice::from_ref(event));
                }
                from = last + 1;
            }
        }
        self.mark = self.spill.borrow().raws();
        self.start = self.spots.len();
        Ok(())
    }

    /// The session being read could not be: what its attempts took is let
    /// go, and a note of why, at `at`, stands in its place.
    pub(crate) fn failed(&mut self, at: Timestamp, text: String) -> Result<(), String> {
        self.again()?;
        self.note(at, text);
        Ok(())
    }

    /// A note of what could not be read, at `at`, where the history is now.
    pub(crate) fn note(&mut self, at: Timestamp, text: String) {
        self.hold(super::note(at, text));
    }

    /// `event` held, next in the history; no session is being read.
    fn hold(&mut self, event: SessionEvent) {
        self.latest = self.latest.max(Some(event.at));
        self.spots.push(placement::spot(&event));
        self.refs.push(HELD | self.held.len() as u32);
        self.held.push(event);
        self.start = self.spots.len();
    }

    /// The history read, placed in the World's turns of `recorded`
    /// (`placement::decide`, `finish`), each event's context restored
    /// (`restore`) and shown as a history shows it (`talk::shown`, the
    /// attachments delivered with words by `receipts`), kept in the store
    /// in that order with its key for merging (`merging::key`) with
    /// `merged`, what of the World's record it is merged with — all of it,
    /// or from where a window begins; an event too large for a record
    /// offered to `early` to put its object now. Those that `told` takes,
    /// as placed, are kept apart too.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn place(
        self,
        world: Placing<'_>,
        restore: &mut dyn FnMut(&mut SessionEvent) -> Result<(), String>,
        receipts: &dyn Receipts,
        early: Option<&Early>,
        told: &dyn Fn(&SessionEvent) -> bool,
    ) -> Result<Placed, String> {
        let Reading { spill, spots, refs, held, .. } = self;
        let mut spill = spill.into_inner();
        spill.flush()?;
        let (turns, sent, mut names) = match world {
            Placing::Held { recorded, merged } => {
                let sent: HashSet<String> = merged
                    .iter()
                    .filter_map(|event| match &event.happened {
                        Happened::Heard { turn: Some(turn), .. } => Some(turn.clone()),
                        _ => None,
                    })
                    .collect();
                (Turns::of(recorded), sent, merging::Names::over(merged))
            }
            Placing::Kept { turns, sent, names } => (turns, sent, names),
        };
        // Placed whole first and kept so: what placing looks at is let go
        // before the events are taken and keyed, and the two are never held
        // at once.
        for (index, turn) in placement::decide(&spots, &turns) {
            spill.push_placed(index as u32, turn.map(|turn| turn as u32))?;
        }
        drop(spots);
        let mut keys = merging::HostKeys::default();
        // Placed by time, the events come mostly in the order kept: read
        // back a batch at a time.
        let mut ahead = crate::timeline::spill::Ahead::default();
        let (mut taken, mut told_events) = (0, Vec::new());
        loop {
            let placed = spill.placed_from(taken)?;
            if placed.is_empty() {
                break;
            }
            taken += placed.len();
            for (index, turn) in placed {
                let (index, turn) = (index as usize, turn.map(|turn| turn as usize));
                let place = refs[index];
                let event = if place & HELD != 0 {
                    held[(place & !HELD) as usize].clone()
                } else {
                    let end = spill.raws();
                    ahead.take(place as usize, &mut |from, alone| {
                        let end = if alone { from as u32 + 1 } else { end };
                        Ok(spill.raws_from(from as u32, end)?.into_iter().map(|(_, event)| event).collect())
                    })?
                };
                let Some(mut event) = placement::finish(event, turn.map(|turn| turns.name(turn)), &turns) else { continue };
                if told(&event) {
                    told_events.push(event.clone());
                }
                restore(&mut event)?;
                for shown in crate::talk::shown(event, receipts, "claude", &|turn| sent.contains(turn)) {
                    let key = merging::key(&shown, &mut names);
                    keys.push(&key);
                    let packed = pack_event(&shown)?;
                    // As a Codex reading offers its own (`codex::follow::Reading`).
                    if let Some(early) = early
                        && packed.len() >= fastctx_content::RECORD_BYTES / 8
                    {
                        let encoded = serde_json::to_vec(&shown).map_err(|error| error.to_string())?;
                        if encoded.len() >= fastctx_content::RECORD_BYTES {
                            early.offer(&encoded);
                        }
                    }
                    spill.push_hosted(packed)?;
                }
            }
        }
        spill.flush()?;
        Ok(Placed { spill, keys, names, told: told_events })
    }
}

/// `event`, of the attempt under way, first through `images`, kept in the
/// store, with what placing looks at of it and where it is kept.
fn keep(
    spill: &RefCell<Spill>,
    spots: &mut Spots,
    refs: &mut Vec<u32>,
    trying: &mut Option<Timestamp>,
    images: &mut dyn FnMut(&mut SessionEvent),
    mut event: SessionEvent,
) -> std::io::Result<()> {
    images(&mut event);
    *trying = (*trying).max(Some(event.at));
    let spot = placement::spot(&event);
    let packed = pack_event(&event).map_err(std::io::Error::other)?;
    let number = spill.borrow_mut().push_raw(packed).map_err(std::io::Error::other)?;
    if number >= HELD {
        return Err(std::io::Error::other("more events than a reading counts"));
    }
    spots.push(spot);
    refs.push(number);
    Ok(())
}
