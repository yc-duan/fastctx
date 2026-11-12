//! A Codex resident's thread followed for its timeline (sync-v2-impl-design
//! §10, §21): read past the turn it was read through (`fastctx_hosts::codex::
//! follow`), the seam moved only where every turn the World saw begin before
//! it had ended. A thread not kept in pages, or one with nothing to read
//! yet, is read whole as ever.

use crate::Node;
use fastctx_hosts::codex::follow::Seam;
use fastctx_types::residents::About;
use fastctx_types::session::SessionEvent;
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;
use std::sync::Arc;

/// What a thread shows past where it was read through.
pub(crate) struct Window {
    /// The host's events of the turns from the seam on, with the thread's
    /// title.
    pub events: Vec<SessionEvent>,
    /// Read from its start: the window is the whole history.
    pub whole: bool,
    /// From when on events belong to the window; the end of time when
    /// nothing lies past the seam.
    pub since: Timestamp,
    /// Where the thread is read through now.
    pub seam: Seam,
    /// Before when the events are settled from now on.
    pub settled: Timestamp,
}

/// The window of `resident`'s thread past `from`, placed against the turns
/// of `recorded`; none where it is not followed.
pub(crate) async fn window(
    node: &Arc<Node>,
    resident: &ResidentId,
    about: &About,
    recorded: &[SessionEvent],
    from: Option<Seam>,
) -> Result<Option<Window>, String> {
    let Some(thread) = about.session.clone() else { return Ok(None) };
    crate::homes::available(node, about).map_err(|failure| failure.message)?;
    let codex = node.codex.of(node, about).map_err(|failure| failure.message)?;
    let owner = Address { machine: node.machine(), resident: resident.clone() };
    let turns = crate::claude::history::turns(recorded);
    let accept = move |at: Timestamp| turns.iter().all(|(start, end)| *start >= at || end.is_some_and(|end| end <= at));
    let seam = from.unwrap_or_default();
    let Some((tail, restarted)) = codex.follow(&thread, &owner, &seam, &accept).await? else { return Ok(None) };
    if let Some(why) = restarted {
        node.log.line(format_args!("{resident}'s thread is read from its start: {why}"));
    }
    let since = if tail.whole { Timestamp::MIN } else { tail.since.unwrap_or(Timestamp::MAX) };
    let settled = tail.settled.unwrap_or(since);
    Ok(Some(Window { events: tail.events, whole: tail.whole, since, seam: tail.seam, settled }))
}
