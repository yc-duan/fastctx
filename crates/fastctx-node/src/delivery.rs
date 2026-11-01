//! Durable handovers and terminal outcomes. These records precede changes
//! to the inbox, `now`, and outbox so each of those views can be repaired.

use std::io;
#[cfg(feature = "storage-drill")]
pub(crate) mod drill;
pub(crate) mod handovers;
mod migrate_handovers;
mod migrate_terminals;
pub(crate) mod recover_outgoing;
mod storage;
pub(crate) mod terminals;
use crate::record_value as value;
pub(crate) mod work;
use storage::{change, read_rows};

use fastctx_types::layout::ConversationDir;
use fastctx_types::residents::Pending;
use fastctx_types::session::{SessionEvent, TurnEnd};
use fastctx_types::{Address, NoticeId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Handover {
    pub id: NoticeId,
    pub inputs: Vec<Pending>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<crate::context::Receipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_input: Option<crate::context::input::Stamp>,
    pub turn: Option<String>,
    pub unresolved: Option<TurnEnd>,
    #[serde(default = "attempting")]
    pub attempting: bool,
    #[serde(skip)]
    pub(crate) revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finished {
    pub turn: String,
    /// The host-reported end, when supplied; `at` remains the settlement time.
    #[serde(default)]
    pub ended_at: Option<Timestamp>,
    pub at: Timestamp,
    pub end: TurnEnd,
    pub readers: Vec<Address>,
    /// Whether this terminal outcome owns a queue transition, fixed on first publication.
    #[serde(default = "legacy_queue")]
    pub settles_queue: bool,
    /// Orders terminal queue transitions without depending on the wall clock.
    #[serde(default)]
    pub queue_order: u64,
}

fn legacy_queue() -> bool {
    // Published beta.2 terminals always settled their composer's queue.
    true
}

pub fn begin(directory: &ConversationDir, inputs: Vec<Pending>) -> io::Result<Handover> {
    begin_with_context(directory, inputs, None, None)
}

pub fn begin_with_context(
    directory: &ConversationDir,
    inputs: Vec<Pending>,
    context: Option<crate::context::Receipt>,
    context_input: Option<crate::context::input::Stamp>,
) -> io::Result<Handover> {
    crate::attachments::delivered::prepare(directory)?;
    let ids: Vec<&str> = inputs.iter().map(|input| input.delivery.id.as_str()).collect();
    let id = crate::speech::fixed_id("handover", &ids);
    storage::initialize(directory)?;
    let handover = Handover {
        id,
        inputs,
        context,
        context_input,
        turn: None,
        unresolved: None,
        attempting: true,
        revision: String::new(),
    };
    let inputs = handover.inputs.clone();
    change(directory, serde_json::to_vec(&inputs)?.len(), move |rows| {
        crate::attachments::delivered::record_in(rows, &inputs)
    })?;
    handovers::put(directory, handover, None)
}

fn attempting() -> bool {
    true
}

pub fn refused(directory: &ConversationDir, handover: &mut Handover) -> io::Result<()> {
    handovers::update(directory, handover, handovers::Change::Refused)
}
pub fn accept(directory: &ConversationDir, handover: &mut Handover, turn: &str) -> io::Result<()> {
    handovers::update(directory, handover, handovers::Change::Accepted(turn.to_string()))
}
pub fn unresolved(directory: &ConversationDir, handover: &mut Handover, end: TurnEnd) -> io::Result<()> {
    handovers::update(directory, handover, handovers::Change::Unresolved(end))
}
pub fn input_turn(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<String>> {
    read_rows(directory, |rows| handovers::input_state(rows, id).map(|state| state.0))
}
pub fn input_uncertainty(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<String>> {
    read_rows(directory, |rows| handovers::input_state(rows, id).map(|state| state.1))
}
pub(crate) fn covers_input(directory: &ConversationDir, id: &NoticeId) -> io::Result<bool> {
    read_rows(directory, |rows| handovers::input_state(rows, id).map(|state| state.2))
}

/// Compatibility projection used by the record-family drill.
#[cfg(feature = "storage-drill")]
pub fn finish_at(
    directory: &ConversationDir,
    turn: &str,
    end: TurnEnd,
    readers: &[Address],
    ended_at: Option<Timestamp>,
    settles_queue: bool,
) -> io::Result<(Finished, bool)> {
    storage::initialize(directory)?;
    let result = terminals::publish_blocking(
        directory,
        turn,
        terminals::Initial { end, ended_at, settles_queue, imported: None, usage: None },
        readers.to_vec(),
    )?;
    let finished =
        terminal(directory, &result.header.turn)?.ok_or_else(|| io::Error::other("published terminal is missing"))?;
    Ok((finished, result.first))
}
pub fn terminals(directory: &ConversationDir) -> io::Result<Vec<Finished>> {
    storage::initialize(directory)?;
    let mut after = 0;
    let mut result = Vec::new();
    loop {
        let page = crate::records::read_background(directory, |rows| {
            rows.page::<terminals::Header>(terminals::FAMILY, after, 32)
        })?;
        let Some(last) = page.last() else { break };
        after = last.0;
        for (_, header) in page {
            result.push(terminal(directory, &header.turn)?.ok_or_else(|| io::Error::other("terminal disappeared"))?);
        }
    }
    Ok(result)
}

pub fn remember_session(directory: &ConversationDir, turn: &str, session: &str) -> io::Result<()> {
    let (key, session) = (crate::speech::fixed_id("turn", &[turn]), session.to_string());
    change(directory, session.len(), move |rows| {
        if rows.get::<String>("turn-sessions", key.as_str())?.is_some_and(|prior| prior != session) {
            return Err(fastctx_storage::Error::new("turn belongs to a different conversation"));
        }
        rows.put("turn-sessions", key.as_str(), &session)
    })
}
pub fn session(directory: &ConversationDir, turn: &str) -> io::Result<Option<String>> {
    let key = crate::speech::fixed_id("turn", &[turn]);
    read_rows(directory, |rows| rows.get("turn-sessions", key.as_str()))
}

/// Whether this event identity has already been durably published.
pub(crate) fn has_event(directory: &ConversationDir, key: &str) -> io::Result<bool> {
    let key = crate::speech::fixed_id("event", &[key]);
    crate::session_record::contains(directory, key.as_str())
}

pub fn event(directory: &ConversationDir, key: &str, event: &SessionEvent) -> io::Result<bool> {
    let key = crate::speech::fixed_id("event", &[key]);
    crate::session_record::append(directory, key.as_str(), event)
}

pub fn history(directory: &ConversationDir) -> io::Result<Vec<SessionEvent>> {
    crate::session_record::history(directory)
}

pub fn handover(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<Handover>> {
    read_rows(directory, |rows| handovers::get(rows, id))
}

/// The write time of the durable intent, retained when importing old files.
pub fn written_before(
    directory: &ConversationDir,
    id: &NoticeId,
    before: Option<std::time::SystemTime>,
) -> io::Result<bool> {
    let Some(before) = before else { return Ok(true) };
    let before = Timestamp::try_from(before).map_err(io::Error::other)?;
    Ok(handovers::modified(directory, id)?.is_some_and(|at| at < before))
}
pub fn terminal(directory: &ConversationDir, turn: &str) -> io::Result<Option<Finished>> {
    storage::initialize(directory)?;
    Ok(terminals::read(directory, turn)?.map(|(header, end, readers)| Finished {
        turn: header.turn,
        ended_at: header.ended_at,
        at: header.at,
        end,
        readers,
        settles_queue: header.settles_queue,
        queue_order: header.queue_order,
    }))
}

/// Prepare legacy delivery families before another record joins their transaction.
pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    storage::initialize(directory)
}

/// A small terminal presence check for transactions that close observations.
pub(crate) fn has_terminal_in(rows: &crate::records::Rows<'_>, turn: &str) -> fastctx_storage::Result<bool> {
    Ok(rows.exists(terminals::FAMILY, turn)? || rows.exists("terminals", turn)?)
}

/// Read immutable contents without walking the reader membership.
pub(crate) fn outcome(directory: &ConversationDir, turn: &str) -> io::Result<Option<(terminals::Header, TurnEnd)>> {
    storage::initialize(directory)?;
    crate::records::read(directory, |rows| {
        terminals::header(rows, turn)?
            .map(|header| {
                let end = terminals::end(rows, &header)?;
                Ok((header, end))
            })
            .transpose()
    })
}
pub(crate) async fn terminal_header(directory: &ConversationDir, turn: &str) -> io::Result<Option<terminals::Header>> {
    let (directory, turn) = (directory.clone(), turn.to_string());
    fastctx_platform::task::spawn_blocking(move || {
        storage::initialize(&directory)?;
        crate::records::read(&directory, |rows| terminals::header(rows, &turn))
    })
    .await
    .map_err(io::Error::other)?
}

/// Run a conversation-local record operation on a thread allowed to wait for IO.
pub(crate) async fn local<T: Send + 'static>(
    directory: &ConversationDir,
    action: impl FnOnce(&ConversationDir) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let directory = directory.clone();
    fastctx_platform::task::spawn_blocking(move || action(&directory)).await.map_err(io::Error::other)?
}

/// Publish an input snapshot while retaining the caller's speech permit through
/// the actual write, including cancellation of the awaiting task.
pub(crate) async fn begin_held(
    node: &crate::Node,
    resident: &fastctx_types::ResidentId,
    directory: &ConversationDir,
    inputs: Vec<Pending>,
) -> io::Result<Handover> {
    let held = node.residents.keep_speaking(resident)?;
    local(directory, move |directory| {
        let _held = held;
        begin(directory, inputs)
    })
    .await
}
