//! A Claude resident's record followed for its timeline (sync-v2-impl-design
//! §10): read past where it was read through, what lies there projected and
//! placed in the World's turns, and the seam moved to where every turn the
//! World saw begin before it had ended. A resident of several sessions, or
//! one whose turns the World cut back, is read whole as ever.

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
    from: Option<(String, Seam)>,
) -> Result<Option<Window>, String> {
    let (mut sessions, problems) = {
        let (reading, whose) = (node.clone(), resident.clone());
        fastctx_platform::task::blocking(move || sessions(&reading, &whose)).await
    };
    if !problems.is_empty() || sessions.len() != 1 {
        return Ok(None);
    }
    let Session { id, began, kept, .. } = sessions.remove(0);
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
    // A seam goes only where every turn the World saw begin before it ended
    // by then: before it, each event's turn is settled (`place`).
    let turns = Arc::new(turns(recorded));
    let mut unsteady = 0;
    let tail = loop {
        let (reading, from, turns) = (path.clone(), seam.clone(), turns.clone());
        let read = fastctx_platform::task::blocking(move || {
            let accept = |at: Timestamp| turns.iter().all(|(start, end)| *start >= at || end.is_some_and(|end| end <= at));
            follow::tail(&reading, &from, &HashSet::new(), &accept)
        })
        .await;
        match read {
            Ok(Ok(tail)) => break tail,
            Ok(Err(Restart::Unsteady)) if unsteady < UNSTEADY_MOST => {
                unsteady += 1;
                tokio::time::sleep(Duration::from_millis(100) * unsteady).await;
            }
            Ok(Err(why)) if seam.identity.is_some() => {
                node.log.line(format_args!("{resident}'s record is read from its start: {why}"));
                seam = Seam::default();
            }
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
    let follow::Tail { transcript, .. } = tail;
    let (events, _) = session_events(node, resident, &id, began, transcript.title.clone(), transcript.events, &transcript.problems);
    let events = place(events, recorded);
    Ok(Some(Window { session: id, events, whole, since, seam: next, settled, kept, kept_here }))
}
