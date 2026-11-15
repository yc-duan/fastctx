//! A Claude resident's record followed for its timeline (sync-v2-impl-design
//! §10): read past where it was read through, what lies there projected and
//! placed in the World's turns, and the seam moved to where every turn the
//! World saw begin before it had ended. Of a resident of several sessions,
//! the last is followed while the earlier ones stay as they were; one whose
//! turns the World cut back is read whole as ever.

use super::{Session, kept, place, session_events, sessions};
use crate::Node;
use crate::timeline::Early;
use crate::timeline::spill::Spill;
use fastctx_hosts::claude::exchange;
use fastctx_hosts::claude::transcript::follow::{self, Restart, Seam};
use fastctx_types::ResidentId;
use fastctx_types::session::{Happened, SessionEvent};
use jiff::Timestamp;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

/// How many times a tail that did not read the same twice is read again
/// before the record is read from its start.
const UNSTEADY_MOST: u32 = 8;

/// What a record shows past where it was read through.
pub(crate) struct Window {
    /// The session followed.
    pub session: String,
    /// The events past the seam, placed in the World's turns, with the
    /// session's title and the note of what could not be read.
    pub events: Vec<SessionEvent>,
    /// Those two of them, as placed: told again at every reading, whatever
    /// their time (EE01).
    pub told: Vec<SessionEvent>,
    /// Read from its start: the window is the whole history.
    pub whole: bool,
    /// From when on events belong to the window: those before stay as they
    /// were written. The end of time when nothing lies past the seam.
    pub since: Timestamp,
    /// Where the record is read through now.
    pub seam: Seam,
    /// Before when the events are settled from now on: the time of the row
    /// the seam moved to, or where the window began when it did not move.
    pub settled: Timestamp,
    /// The last record the session's mod saw kept, and whether it is past
    /// the seam in the file.
    pub kept: Option<String>,
    pub kept_here: bool,
    /// The sessions before the one followed, as their files stood: a
    /// window is read past the seam only while they stand so.
    pub earlier: Vec<Stamp>,
    /// The record ids of the rows the seam moved past in this reading (all
    /// before it, read whole), as 8-byte digests (`PrefixIds::digest`).
    pub prefix_ids: Vec<u64>,
    /// Read from its start into a work store (design §28.51 ⑤-5): the
    /// history placed there (`events` is empty) and the digests of the
    /// record ids before the seam (`prefix_ids` is empty), with what put
    /// the objects of its events as they were read.
    pub held: Option<(kept::Placed, Option<Early>)>,
}

/// Where a reading of the record from its start is kept rather than held
/// (design §28.51 ⑤-5): the work store, and the library the objects of its
/// large events are put into as they are read (`Early`, begun only for a
/// reading kept).
pub(crate) struct Keep {
    pub spill: Spill,
    pub library: fastctx_content::v2::Library,
    /// Where the record ids before the seam are kept beside the checkpoint
    /// (design §28.51 ⑤-7c): their name and generation, asked of a batch
    /// at a time; none where none are.
    pub seen: Option<(String, u64)>,
}

/// A reading kept on disk, shared by the steps that read into it.
type Shared = Arc<std::sync::Mutex<kept::Reading>>;

/// The record ids of a followed record's rows before its seam, as 8-byte
/// digests in order: a row of one of them again past the seam reads the
/// record whole (CC09).
#[derive(Default)]
pub(crate) struct PrefixIds(pub Vec<u64>);

impl PrefixIds {
    pub(crate) fn digest(id: &str) -> u64 {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(id.as_bytes());
        u64::from_le_bytes(digest[..8].try_into().expect("eight bytes"))
    }

    pub(crate) fn holds(&self, id: &str) -> bool {
        self.0.binary_search(&Self::digest(id)).is_ok()
    }
}

/// A session's record file as it stood: the session, its length, and when
/// it last changed.
pub(crate) type Stamp = (String, u64, i128);

/// The stamps of `sessions`' record files, none where one cannot be told.
fn stamps(node: &Node, sessions: &[Session]) -> Option<Vec<Stamp>> {
    sessions
        .iter()
        .map(|session| {
            let directory = exchange::sessions(node.world.path()).join(&session.id);
            if !crate::claude::rewind::cuts(&directory).ok()?.is_empty() {
                return None;
            }
            let path = exchange::transcript(&directory).ok()??;
            let metadata = fastctx_platform::disk::metadata(&path).ok()?;
            let modified = metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos() as i128;
            Some((session.id.clone(), metadata.len(), modified))
        })
        .collect()
}

/// Where a seam may go, of the World's turns (`turns`): at a time no turn
/// the World saw begin before is still under way — every one began at it
/// or after, or ended by then. Asked of each place a seam might go, so the
/// turns are laid out once to be looked up (design §28.51 ⑤-7 review), not
/// gone over for each: the spans of those that ended, open at both ends,
/// joined where they overlap — not where one ends as the next begins,
/// which leaves that moment free — and the first start of those under way.
pub(crate) struct Accepting {
    spans: Vec<(Timestamp, Timestamp)>,
    open: Option<Timestamp>,
}

impl Accepting {
    pub(crate) fn of(turns: &[(Timestamp, Option<Timestamp>)]) -> Self {
        let open = turns.iter().filter(|(_, end)| end.is_none()).map(|(start, _)| *start).min();
        let mut ended: Vec<(Timestamp, Timestamp)> = turns.iter().filter_map(|(start, end)| end.filter(|end| end > start).map(|end| (*start, end))).collect();
        ended.sort_unstable();
        let mut spans: Vec<(Timestamp, Timestamp)> = Vec::with_capacity(ended.len());
        for (start, end) in ended {
            match spans.last_mut() {
                Some((_, last)) if start < *last => *last = (*last).max(end),
                _ => spans.push((start, end)),
            }
        }
        Accepting { spans, open }
    }

    /// Whether a seam may go at `at`: no turn began before it and is under
    /// way at it.
    pub(crate) fn accepts(&self, at: Timestamp) -> bool {
        if self.open.is_some_and(|start| start < at) {
            return false;
        }
        let before = self.spans.partition_point(|(start, _)| *start < at);
        before.checked_sub(1).is_none_or(|last| self.spans[last].1 <= at)
    }
}

/// The turns of `recorded`: when each began, and ended if it did.
#[cfg(test)]
pub(crate) fn turns(recorded: &[SessionEvent]) -> Vec<(Timestamp, Option<Timestamp>)> {
    let mut ends: std::collections::HashMap<&str, Timestamp> = std::collections::HashMap::new();
    for event in recorded {
        if let Happened::TurnEnded { turn, .. } = &event.happened {
            ends.insert(turn, event.at);
        }
    }
    recorded
        .iter()
        .filter_map(|event| match &event.happened {
            Happened::TurnStarted { turn } => Some((event.at, ends.get(turn.as_str()).copied())),
            _ => None,
        })
        .collect()
}

/// The window of `resident`'s record past `from`, the session and seam it
/// was read through, placed in the turns of the World's record (`world`,
/// kept on disk, design §28.51 ⑥-1); none where it is not followed (several
/// sessions, turns cut back), so it is read whole as ever. Read from its
/// start, it is read into `keep` where there is one: the earlier sessions
/// and the last, placed there (`Window::held`).
pub(crate) async fn window(
    node: &Arc<Node>,
    resident: &ResidentId,
    world: &Arc<std::sync::Mutex<crate::timeline::world::World>>,
    from: Option<(String, Seam, Vec<Stamp>)>,
    prefix: Arc<PrefixIds>,
    keep: Option<Keep>,
) -> Result<Option<Window>, String> {
    let (mut sessions, problems) = {
        let (reading, whose) = (node.clone(), resident.clone());
        fastctx_platform::task::blocking(move || sessions(&reading, &whose)).await
    };
    let Some(Session { id, began, kept, process }) = sessions.pop().filter(|_| problems.is_empty()) else { return Ok(None) };
    // Its process gone, a last row without its line's end will never get one.
    let ended = process.as_ref().is_some_and(|process| fastctx_platform::has_ended(process).unwrap_or(false));
    // The sessions before the last as their files stand: unchanged since the
    // seam, they stay as written; else all is read again.
    let earlier = {
        let (reading, before) = (node.clone(), std::mem::take(&mut sessions));
        let (stamped, before) = fastctx_platform::task::blocking(move || (stamps(&reading, &before), before)).await;
        sessions = before;
        match stamped {
            Some(stamped) => stamped,
            None => return Ok(None),
        }
    };
    let from = from.filter(|(_, _, stood)| *stood == earlier).map(|(session, seam, _)| (session, seam));
    let directory = exchange::sessions(node.world.path()).join(&id);
    let (cuts, path) = fastctx_platform::task::blocking(move || {
        let cuts = crate::claude::rewind::cuts(&directory).map_err(|error| error.to_string())?;
        let path = exchange::transcript(&directory)
            .map_err(|error| format!("where it is kept could not be read: {error}"))?
            .ok_or("the World's plugin never said where Claude Code keeps it")?;
        Ok::<_, String>((cuts, path))
    })
    .await?;
    if !cuts.is_empty() {
        return Ok(None);
    }
    let mut seam = from.filter(|(session, _)| *session == id).map(|(_, seam)| seam).unwrap_or_default();
    // Read from its start, the last session shows what it copied from the
    // earlier ones only there (`continuing`): by their records' ids, read
    // with them, whole. A seam still at its start reads it from there: as
    // a first reading does, with the earlier sessions.
    let several = !sessions.is_empty();
    if several && seam.offset == 0 {
        seam = Seam::default();
    }
    // The record ids before the seam, asked of where they are kept.
    let seen_kept = keep.as_ref().and_then(|keep| keep.seen.clone().map(|(name, generation)| (keep.library.clone(), name, generation)));
    let (mut keep, mut early, mut reading) = (keep, None, None);
    let (mut before, mut copied) = (Vec::new(), HashSet::new());
    if seam.identity.is_none() {
        reading = begin(&mut keep, &mut early, resident).await?;
        match &reading {
            // The sessions before the last read into the same store, as
            // `earlier` reads them: all of each, or all is read as ever.
            Some(held) if several => {
                if !earlier_kept(node, resident, std::mem::take(&mut sessions), held).await? {
                    return Ok(None);
                }
                // As below (DD05).
                seam.latest = held.lock().expect("never poisoned").latest();
            }
            Some(_) => {}
            None if several => {
                let Some(read) = super::earlier(node, resident, sessions).await else { return Ok(None) };
                (before, copied) = read;
                // The last session's seam goes only past every earlier
                // session's events too: one at a moment of theirs would be
                // named by an order the window does not see (DD05).
                seam.latest = before.iter().map(|event| event.at).max();
            }
            None => {}
        }
    }
    // A seam goes only where every turn the World saw begin before it ended
    // by then: before it, each event's turn is settled (`place`).
    let turns = Arc::new(Accepting::of(&world.lock().unwrap_or_else(std::sync::PoisonError::into_inner).turn_spans()));
    let asked: Vec<String> = kept.iter().cloned().collect();
    let mut unsteady = 0;
    let (tail, mut prefix_ids, read_kept) = loop {
        // Kept where it can be, past a seam too (design §28.51 ⑤-7b): a
        // seam that cannot move leaves a window as long as the record.
        if reading.is_none() {
            reading = begin(&mut keep, &mut early, resident).await?;
        }
        let (from, turns, prefix) = (seam.clone(), turns.clone(), prefix.clone());
        let into = reading.clone();
        let read_kept = into.is_some();
        let read = match into {
            Some(held) => {
                let (path, node, whose, asked, seen_kept) = (path.clone(), node.clone(), resident.clone(), asked.clone(), seen_kept.clone());
                fastctx_platform::task::blocking(move || {
                    let accept = |at: Timestamp| turns.accepts(at);
                    let seen = |uuids: &[String]| -> std::io::Result<HashSet<String>> {
                        let Some((library, name, generation)) = &seen_kept else {
                            return Ok(uuids.iter().filter(|uuid| prefix.holds(uuid)).cloned().collect());
                        };
                        let digests: Vec<u64> = uuids.iter().map(|uuid| PrefixIds::digest(uuid)).collect();
                        let held = library.digests_among(name, *generation, &digests).map_err(std::io::Error::other)?;
                        Ok(uuids.iter().zip(&digests).filter(|(_, digest)| held.contains(digest)).map(|(uuid, _)| uuid.clone()).collect())
                    };
                    // The record ids the seam moves past kept in the store
                    // by their digests, not held.
                    let read = held.lock().expect("never poisoned").tail(
                        &path,
                        &from,
                        &accept,
                        ended,
                        &seen,
                        &asked,
                        &PrefixIds::digest,
                        &mut |event| crate::user_images::claude(&node, &whose, event),
                    );
                    read.map(|read| read.map(|tail| (tail, Vec::new())))
                })
                .await
            }
            None => {
                let reading = path.clone();
                let copied = if from.identity.is_none() { copied.clone() } else { HashSet::new() };
                fastctx_platform::task::blocking(move || {
                    let accept = |at: Timestamp| turns.accepts(at);
                    let read = follow::tail(&reading, &from, &copied, &accept, ended, &|id: &str| prefix.holds(id));
                    read.map(|read| {
                        read.map(|tail| {
                            let digests = tail.prefix_ids.iter().map(|id| PrefixIds::digest(id)).collect();
                            (tail, digests)
                        })
                    })
                })
                .await
            }
        };
        match read {
            Ok(Ok((tail, digests))) => break (tail, digests, read_kept),
            Ok(Err(Restart::Unsteady)) if unsteady < UNSTEADY_MOST => {
                unsteady += 1;
                tokio::time::sleep(Duration::from_millis(100) * unsteady).await;
            }
            // Read from its start, the earlier sessions are read with it.
            Ok(Err(why)) if seam.identity.is_some() => {
                node.log.line(format_args!("{resident}'s record is read from its start: {why}"));
                if several {
                    return Ok(None);
                }
                seam = Seam::default();
            }
            // Not to be read past a seam from its start either — a last row
            // only a whole reading takes, say: read whole as ever (DD02).
            Ok(Err(Restart::Unsupported(_))) => return Ok(None),
            Ok(Err(why)) => return Err(format!("{}: {why}", path.display())),
            // Gone — Claude Code removes old sessions itself — it is read
            // whole as ever: what the World saw of it, and that it is gone.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    };
    // Stopped before a line larger than World reads (§28.49): not followed
    // past a seam, read whole as ever, where a reading short of the record
    // takes nothing away and is tried again (`hosted`).
    if tail.transcript.refused.is_some() {
        return Ok(None);
    }
    let whole = seam.identity.is_none();
    let kept_here = kept.as_ref().is_some_and(|kept| tail.transcript.uuids.contains(kept));
    let since = if whole { Timestamp::MIN } else { tail.since.unwrap_or(Timestamp::MAX) };
    let settled = tail.seam_at.unwrap_or(since);
    let next = tail.seam.clone().unwrap_or(seam);
    let follow::Tail { transcript, .. } = tail;
    // Read from its start after earlier sessions, their record ids are kept
    // with those before the seam: one of them the last session copies late,
    // past the seam, reads all whole, as a whole reading leaves it out
    // (EE04).
    if whole && several {
        prefix_ids.extend(copied.iter().map(|id| PrefixIds::digest(id)));
    }
    let title = transcript.title.clone();
    let problem = super::problem_note(&id, &transcript.problems, transcript.problems_omitted);
    if let Some(held) = reading.filter(|_| read_kept) {
        // Accepted, placed in the World's turns and shown as a history shows
        // it there, as `hosted_kept` places its sessions — merged with what
        // the World recorded from where the window begins, as `produce`
        // merges a window held in memory.
        let (directory, session, world) = (node.world.conversation(resident), id.clone(), world.clone());
        let held = fastctx_platform::task::blocking(move || {
            let mut reading = Arc::try_unwrap(held).map_err(|_| "the reading is held still".to_string())?.into_inner().expect("never poisoned");
            // As above (EE04), the earlier sessions' ids as they stood
            // before the last's join them, kept with those the seam moved
            // past (`Placed`'s store: `Spill::moved_bytes`).
            if whole && several {
                reading.keep_earlier_moved(&PrefixIds::digest)?;
            }
            reading.accept(&session, began, &transcript)?;
            // Merged with what the World recorded from where the window
            // begins (all of it, read whole), its names and keys kept beside
            // it; placed in all the turns it records.
            let mut world = world.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            world.view((!whole).then_some(since))?;
            let (names, sent) = world.name()?;
            let failed = std::cell::RefCell::new(None);
            let heard = |turn: &str, text: &str| world.heard_in(turn, text).unwrap_or_else(|error| {
                failed.borrow_mut().get_or_insert(error);
                false
            });
            let placed = fastctx_platform::roles::in_place("placing Claude history", || {
                let projection = crate::context::input::Projection::open(&directory).map_err(|error| error.to_string())?;
                let mut restore = |event: &mut SessionEvent| projection.restore(event).map_err(|error| error.to_string());
                let told = |event: &SessionEvent| told(event, began, title.as_deref(), problem.as_deref());
                let world = kept::Placing::Kept { turns: world.turns(&heard), sent, names };
                reading.place(world, &mut restore, &kept::Fresh(directory.clone()), early.as_ref(), &told).map(|placed| (placed, early))
            });
            match failed.into_inner() {
                Some(error) => Err(error),
                None => placed,
            }
        })
        .await?;
        let told = held.0.told.clone();
        return Ok(Some(Window { session: id, events: Vec::new(), told, whole, since, seam: next, settled, kept, kept_here, earlier, prefix_ids, held: Some(held) }));
    }
    let (events, problem) = session_events(node, resident, &id, began, title.clone(), transcript.events, (&transcript.problems, transcript.problems_omitted));
    before.extend(events);
    // Held whole, as the history read is.
    let recorded = {
        let world = world.clone();
        fastctx_platform::task::blocking(move || {
            let mut world = world.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            world.view(None)?;
            world.events()
        })
        .await?
    };
    let events = place(before, &recorded);
    let told = events.iter().filter(|event| told(event, began, title.as_deref(), problem.as_deref())).cloned().collect();
    Ok(Some(Window { session: id, events, told, whole, since, seam: next, settled, kept, kept_here, earlier, prefix_ids, held: None }))
}

/// Whether `event`, as placed, is one a session's reading tells again at
/// every reading (EE01): its title, at `began`, or its note of what could
/// not be read.
fn told(event: &SessionEvent, began: Timestamp, title: Option<&str>, problem: Option<&str>) -> bool {
    match &event.happened {
        Happened::Titled { title: told } => event.at == began && title == Some(told.as_str()),
        Happened::Health { health, .. } => event.at == began && problem == Some(health.text.as_str()),
        _ => false,
    }
}

/// The reading kept in `keep`, begun, where there is one (taken from it):
/// what puts its large events' objects as they are read in `early`.
async fn begin(keep: &mut Option<Keep>, early: &mut Option<Early>, resident: &ResidentId) -> Result<Option<Shared>, String> {
    let Some(Keep { spill, library, .. }) = keep.take() else { return Ok(None) };
    let reading = fastctx_platform::task::blocking(move || kept::Reading::new(spill)).await?;
    *early = Early::new(library, resident);
    Ok(Some(Arc::new(std::sync::Mutex::new(reading))))
}

/// `sessions` read whole into `reading` and accepted there, as `earlier`
/// reads them (`read_whole_kept`): false where one could not be read whole,
/// so that all is read as ever.
async fn earlier_kept(node: &Arc<Node>, resident: &ResidentId, sessions: Vec<Session>, reading: &Shared) -> Result<bool, String> {
    let root = exchange::sessions(node.world.path());
    for Session { id, began, process, kept } in sessions {
        let Ok((read, whole)) = super::read_whole_kept(node, resident, &id, root.join(&id), kept, process, reading).await else { return Ok(false) };
        if !whole {
            return Ok(false);
        }
        let holding = reading.clone();
        fastctx_platform::task::blocking(move || holding.lock().expect("never poisoned").accept(&id, began, &read)).await?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where a seam may go, looked up of the turns laid out, is where it may
    /// go asked of each turn (`turns.iter().all(…)`, as the follower asked
    /// it before): over turns that overlap, touch, end as they begin or
    /// before, and are under way.
    #[test]
    fn a_seam_goes_where_every_turn_lets_it() {
        let mut state = 0x243f_6a88_85a3_08d3u64;
        let mut pick = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % n
        };
        let at = |second: u64| Timestamp::from_second(1_800_000_000 + second as i64).unwrap();
        for round in 0..20_000 {
            let turns: Vec<(Timestamp, Option<Timestamp>)> = (0..pick(8))
                .map(|_| {
                    let start = pick(30);
                    let end = match pick(5) {
                        0 => None,
                        1 => Some(start.saturating_sub(pick(3))),
                        _ => Some(start + pick(10)),
                    };
                    (at(start), end.map(at))
                })
                .collect();
            let accepting = Accepting::of(&turns);
            for second in 0..42 {
                let asked = at(second);
                let each = turns.iter().all(|(start, end)| *start >= asked || end.is_some_and(|end| end <= asked));
                assert_eq!(accepting.accepts(asked), each, "round {round}, at {second}, {turns:?}");
            }
        }
    }
}
