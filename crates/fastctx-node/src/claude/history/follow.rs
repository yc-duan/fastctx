//! A Claude resident's record followed for its timeline (sync-v2-impl-design
//! §10): read past where it was read through, what lies there projected and
//! placed in the World's turns, and the seam moved to where every turn the
//! World saw begin before it had ended. Of a resident of several sessions,
//! the last is followed while the earlier ones stay as they were; one whose
//! turns the World cut back is read whole as ever.

use super::{Session, place, session_events, sessions};
use crate::Node;
use fastctx_hosts::claude::exchange;
use fastctx_hosts::claude::transcript::follow::{self, Restart, Seam};
use fastctx_types::ResidentId;
use fastctx_types::session::{Happened, SessionEvent};
use jiff::Timestamp;
use std::collections::{HashMap, HashSet};
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
    /// before it, read whole).
    pub prefix_ids: Vec<String>,
}

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

/// The turns of `recorded`: when each began, and ended if it did.
pub(crate) fn turns(recorded: &[SessionEvent]) -> Vec<(Timestamp, Option<Timestamp>)> {
    let mut ends: HashMap<&str, Timestamp> = HashMap::new();
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
/// was read through, placed in the turns of `recorded`; none where it is not
/// followed (several sessions, turns cut back), so it is read whole as ever.
pub(crate) async fn window(
    node: &Arc<Node>,
    resident: &ResidentId,
    recorded: &[SessionEvent],
    from: Option<(String, Seam, Vec<Stamp>)>,
    prefix: Arc<PrefixIds>,
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
    // with them, whole.
    let several = !sessions.is_empty();
    let (mut before, mut copied) = (Vec::new(), HashSet::new());
    if seam.identity.is_none() && several {
        let Some(read) = super::earlier(node, resident, sessions).await else { return Ok(None) };
        (before, copied) = read;
        // The last session's seam goes only past every earlier session's
        // events too: one at a moment of theirs would be named by an order
        // the window does not see (DD05).
        seam.latest = before.iter().map(|event| event.at).max();
    }
    // A seam goes only where every turn the World saw begin before it ended
    // by then: before it, each event's turn is settled (`place`).
    let turns = Arc::new(turns(recorded));
    let mut unsteady = 0;
    let tail = loop {
        let (reading, from, turns, prefix) = (path.clone(), seam.clone(), turns.clone(), prefix.clone());
        let copied = if from.identity.is_none() { copied.clone() } else { HashSet::new() };
        let read = fastctx_platform::task::blocking(move || {
            let accept = |at: Timestamp| turns.iter().all(|(start, end)| *start >= at || end.is_some_and(|end| end <= at));
            follow::tail(&reading, &from, &copied, &accept, ended, &|id: &str| prefix.holds(id))
        })
        .await;
        match read {
            Ok(Ok(tail)) => break tail,
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
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("{} is gone; Claude Code removes old sessions itself", path.display()));
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    };
    let whole = seam.identity.is_none();
    let kept_here = kept.as_ref().is_some_and(|kept| tail.transcript.uuids.contains(kept));
    let since = if whole { Timestamp::MIN } else { tail.since.unwrap_or(Timestamp::MAX) };
    let settled = tail.seam_at.unwrap_or(since);
    let next = tail.seam.clone().unwrap_or(seam);
    let follow::Tail { transcript, prefix_ids, .. } = tail;
    let (events, _) = session_events(node, resident, &id, began, transcript.title.clone(), transcript.events, &transcript.problems);
    before.extend(events);
    let events = place(before, recorded);
    Ok(Some(Window { session: id, events, whole, since, seam: next, settled, kept, kept_here, earlier, prefix_ids }))
}
