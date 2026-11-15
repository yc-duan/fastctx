//! A Codex resident's thread followed for its timeline (sync-v2-impl-design
//! §10, §21): read past the turn it was read through (`fastctx_hosts::codex::
//! follow`), the seam moved only where every turn the World saw begin before
//! it had ended. A thread not kept in pages, or one with nothing to read
//! yet, is read whole as ever.

use crate::Node;
use crate::talk::merging;
use crate::timeline::spill::Spill;
use fastctx_hosts::codex::follow::{Keeping, Seam, Timing};
use fastctx_types::residents::About;
use fastctx_types::session::{Happened, SessionEvent};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;
use std::collections::HashSet;
use std::sync::Arc;

/// What a thread shows past where it was read through.
pub(crate) struct Window {
    /// The host's events of the turns from the seam on, with the thread's
    /// title; none where it was read whole into a `Reading`.
    pub events: Vec<SessionEvent>,
    /// Read whole into the `Reading` it was given: the events are there.
    pub kept: bool,
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

/// The window of `resident`'s thread past `from`, placed against the
/// World's turns (`turns`: each one's start and end, as
/// `claude::history::turns` gives them); none where it is not followed.
/// Read whole, it is read into `reading`, where given.
pub(crate) async fn window(
    node: &Arc<Node>,
    resident: &ResidentId,
    about: &About,
    turns: &[(Timestamp, Option<Timestamp>)],
    from: Option<Seam>,
    reading: Option<&mut Reading>,
) -> Result<Option<Window>, String> {
    let Some(thread) = about.session.clone() else { return Ok(None) };
    crate::homes::available(node, about).map_err(|failure| failure.message)?;
    let codex = node.codex.of(node, about).map_err(|failure| failure.message)?;
    let owner = Address { machine: node.machine(), resident: resident.clone() };
    // A seam goes only where every turn begun before it ended by then
    // (`Accepting`: as every turn asked in turn).
    let turns = crate::claude::history::Accepting::of(turns);
    let accept = move |at: Timestamp| turns.accepts(at);
    let seam = from.unwrap_or_default();
    let keeping = reading.map(|reading| reading as &mut dyn Keeping);
    let Some((tail, restarted)) = codex.follow(&thread, &owner, &seam, &accept, keeping).await? else { return Ok(None) };
    if let Some(why) = restarted {
        node.log.line(format_args!("{resident}'s thread is read from its start: {why}"));
    }
    let since = if tail.whole { Timestamp::MIN } else { tail.since.unwrap_or(Timestamp::MAX) };
    let settled = tail.settled.unwrap_or(since);
    Ok(Some(Window { events: tail.events, kept: tail.kept, whole: tail.whole, since, seam: tail.seam, settled }))
}

/// A whole reading of a thread kept on disk as it is read (design §28.51),
/// and what the history makes of each of the host's events as it comes, as
/// `talk::assemble` makes it of them all: the context it was given restored,
/// what a reader is shown of it (`talk::shown`), kept with what the merge
/// decides on (`merging::key`). Beside them, what the timeline takes of the
/// host's own events: those it tells again at every reading, and the turns
/// it has (`watch::recorded`).
pub(crate) struct Reading {
    spill: Spill,
    /// The conversation's files, for the context and what was delivered
    /// with words; none in the bench, which has none.
    directory: Option<fastctx_types::layout::ConversationDir>,
    /// Its context projection opened (`Projection::open`) once it began.
    prepared: bool,
    /// What was delivered with words, where there are the conversation's
    /// files (`attachments::delivered`): looked up afresh for each event, so
    /// that nothing of it builds up over the reading.
    receipts: Option<fastctx_types::layout::ConversationDir>,
    /// The turns the World's record has words it sent into.
    sent: HashSet<String>,
    /// Where the timeline goes, for its objects put as they are read
    /// (`assets::Early`), begun with the reading; none in a bench that has
    /// none.
    library: Option<(fastctx_content::v2::Library, ResidentId)>,
    /// What the World's record names, closed (`merging::Names::over`): each
    /// beginning keys the host's events by it anew. Closed, it numbers no
    /// name more: one, handed on with what was kept (`finish`).
    names: merging::Names,
    kept: Kept,
}

/// What a `Reading` kept, once read.
#[derive(Default)]
pub(crate) struct Kept {
    pub(crate) told: Vec<SessionEvent>,
    pub(crate) turns: HashSet<String>,
    /// The host's events' keys for merging, numbered by names closed over
    /// the World's record (`merging::Names::over`), with their times.
    pub(crate) keys: merging::HostKeys,
    pub(crate) names: merging::Names,
    /// The objects of its events put as they were read.
    pub(crate) early: Option<crate::timeline::Early>,
}

impl Reading {
    /// A reading of `resident`'s thread to be kept in `spill`, beside the
    /// World's record (kept on disk, `timeline::world`): the turns words
    /// were heard in (`sent`) and the names closed over it, of all of it —
    /// one that `watch::recorded` would leave out is of a turn the host has
    /// none of, which no event of the host's asks after. Nothing is opened
    /// until it begins.
    pub(crate) fn kept(node: &Node, resident: &ResidentId, sent: HashSet<String>, names: merging::Names, spill: Spill) -> Self {
        let directory = node.world.conversation(resident);
        let library = node.assets.as_ref().map(|assets| (assets.library.clone(), resident.clone()));
        Self { spill, directory: Some(directory.clone()), prepared: false, receipts: Some(directory), sent, library, names, kept: Kept::default() }
    }

    /// A reading of a thread with no conversation's files, kept in `spill`:
    /// nothing restored, nothing delivered with words, no words the World
    /// sent — as the bench reads one.
    #[cfg(feature = "bench")]
    pub(crate) fn bare(spill: Spill, library: Option<(fastctx_content::v2::Library, ResidentId)>) -> Self {
        let names = merging::Names::over(&[]);
        Self { spill, directory: None, prepared: true, receipts: None, sent: HashSet::new(), library, kept: Kept::default(), names }
    }

    /// What it kept: the store and what was taken of the host's events.
    pub(crate) fn finish(self) -> (Spill, Kept) {
        (self.spill, Kept { names: self.names, ..self.kept })
    }
}

impl Keeping for Reading {
    fn begin(&mut self) -> Result<(), String> {
        // Objects put by a reading begun before are kept: they may be this
        // one's too, and those no write takes are let go at the end.
        let early = self.kept.early.take();
        self.kept = Kept { early, ..Kept::default() };
        if !self.prepared
            && let Some(directory) = &self.directory
        {
            fastctx_platform::roles::in_place("opening a conversation's context projection", || {
                crate::context::input::Projection::open(directory).map(drop)
            })
            .map_err(|error| error.to_string())?;
            self.prepared = true;
        }
        self.spill.begin()?;
        // Begun once the work store is: a new holder being admitted holds
        // both off alike.
        if self.kept.early.is_none()
            && let Some((library, resident)) = &self.library
        {
            self.kept.early = crate::timeline::Early::new(library.clone(), resident);
        }
        Ok(())
    }

    fn time(&mut self, turn: &str, id: &str, timing: Timing) -> Result<(), String> {
        self.spill.time(turn, id, timing)
    }

    fn timing(&mut self, turn: &str, id: &str) -> Result<Option<Timing>, String> {
        self.spill.timing(turn, id)
    }

    fn item(&mut self, turn: &str, entry: serde_json::Value) -> Result<(), String> {
        self.spill.item(turn, &entry)
    }

    fn items_of(&mut self, turn: &str, from: usize, limit: usize) -> Result<Vec<serde_json::Value>, String> {
        self.spill.items_of(turn, from, limit)
    }

    fn entry(&mut self, entry: serde_json::Value) -> Result<(), String> {
        self.spill.entry(&entry)
    }

    fn entries(&mut self, from: usize, limit: usize) -> Result<Vec<serde_json::Value>, String> {
        self.spill.entries(from, limit)
    }

    fn event(&mut self, event: SessionEvent) -> Result<(), String> {
        if crate::timeline::told_again(&event) {
            self.kept.told.push(event.clone());
        }
        if let Some(turn) = crate::talk::turn_of(&event.happened) {
            self.kept.turns.insert(turn.to_string());
        }
        let (directory, receipts, sent) = (self.directory.as_ref(), self.receipts.as_ref(), &self.sent);
        let show = move |mut event: SessionEvent| -> Result<Vec<SessionEvent>, String> {
            if let Some(directory) = directory {
                crate::context::input::Projection::prepared(directory).restore(&mut event).map_err(|error| error.to_string())?;
            }
            Ok(match receipts {
                Some(receipts) => crate::talk::shown(event, &crate::attachments::delivered::at(receipts.clone()), "codex", &|turn| sent.contains(turn)),
                None => crate::talk::shown(event, &fastctx_types::session::attachments::NoReceipts, "codex", &|turn| sent.contains(turn)),
            })
        };
        // Only words reach the conversation's files, for their context and
        // what was delivered with them.
        let shown = if matches!(event.happened, Happened::Heard { .. }) {
            fastctx_platform::roles::in_place("reading what was delivered with words", || show(event))?
        } else {
            show(event)?
        };
        for shown in shown {
            let key = merging::key(&shown, &mut self.names);
            self.kept.keys.push(&key);
            let packed = crate::timeline::spill::pack_event(&shown)?;
            // Too large for a record, its object is put now where it can be:
            // only one that may be is written as JSON to tell (kept, a value
            // is not much smaller than its JSON, nor ever eight times).
            if let Some(early) = &self.kept.early
                && packed.len() >= fastctx_content::RECORD_BYTES / 8
            {
                let encoded = serde_json::to_vec(&shown).map_err(|error| error.to_string())?;
                if encoded.len() >= fastctx_content::RECORD_BYTES {
                    early.offer(&encoded);
                }
            }
            self.spill.push_hosted(packed)?;
        }
        Ok(())
    }
}
