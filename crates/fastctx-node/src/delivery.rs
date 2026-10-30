//! Durable handovers and terminal outcomes. These records precede changes
//! to the inbox, `now`, and outbox so each of those views can be repaired.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
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
    let _transaction = crate::resident_storage::lock();
    // The host's record will name the files by id alone.
    crate::attachments::delivered::record(directory, &inputs)?;
    let ids: Vec<&str> = inputs.iter().map(|input| input.delivery.id.as_str()).collect();
    let id = crate::speech::fixed_id("handover", &ids);
    let handover = Handover { id, inputs, context, context_input, turn: None, unresolved: None, attempting: true };
    let path = handover_path(directory, &handover.id);
    match create(&path, &handover) {
        Ok(()) => Ok(handover),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let mut prior: Handover = read(&path)?;
            accepted_turn(directory, &mut prior)?;
            if prior.turn.is_none() && prior.unresolved.is_none() {
                prior.attempting = true;
                prior.inputs = handover.inputs;
                prior.context = handover.context;
                prior.context_input = handover.context_input;
                write(&path, &prior)?;
            }
            Ok(prior)
        }
        Err(error) => Err(error),
    }
}

fn attempting() -> bool {
    true
}

pub fn refused(directory: &ConversationDir, handover: &mut Handover) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    handover.attempting = false;
    write(&handover_path(directory, &handover.id), handover)
}

pub fn accept(directory: &ConversationDir, handover: &mut Handover, turn: &str) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    if handover.turn.as_deref().is_some_and(|prior| prior != turn) {
        return Err(io::Error::other("one handover cannot belong to two turns"));
    }
    if handover.turn.is_some() {
        return Ok(());
    }
    // Acceptance never replaces the intent file: a concurrent reader on
    // Windows may deny replacement, although a new receipt can be published.
    let path = handover_path(directory, &handover.id).with_extension("accepted");
    match create(&path, &turn) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if read::<String>(&path)? != turn {
                return Err(io::Error::other("one handover cannot belong to two turns"));
            }
        }
        result => result?,
    }
    handover.turn = Some(turn.to_string());
    Ok(())
}

pub fn unresolved(directory: &ConversationDir, handover: &mut Handover, end: TurnEnd) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    if handover.turn.is_some() {
        return Err(io::Error::other("a handover with a known turn is not an unknown submission"));
    }
    if handover.unresolved.is_none() {
        handover.unresolved = Some(end);
        write(&handover_path(directory, &handover.id), handover)?;
    }
    Ok(())
}

fn handovers_here(directory: &ConversationDir) -> io::Result<Vec<Handover>> {
    let mut handovers: Vec<Handover> = list(&directory.handovers())?;
    for handover in &mut handovers {
        accepted_turn(directory, handover)?;
    }
    Ok(handovers)
}

fn accepted_turn(directory: &ConversationDir, handover: &mut Handover) -> io::Result<()> {
    let path = handover_path(directory, &handover.id).with_extension("accepted");
    if let Some(turn) = optional::<String>(&path)? {
        if handover.turn.as_ref().is_some_and(|prior| prior != &turn) {
            return Err(io::Error::other("a handover and its receipt disagree about their turn"));
        }
        handover.turn = Some(turn);
    }
    Ok(())
}

pub fn input_turn(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<String>> {
    let _transaction = crate::resident_storage::lock();
    Ok(handovers_here(directory)?.into_iter().find_map(|handover| {
        handover.inputs.iter().any(|input| input.delivery.id == *id).then_some(handover.turn).flatten()
    }))
}

pub fn input_uncertainty(directory: &ConversationDir, id: &NoticeId) -> io::Result<Option<String>> {
    let _transaction = crate::resident_storage::lock();
    Ok(handovers_here(directory)?.into_iter().find_map(|handover| {
        if handover.inputs.iter().any(|input| input.delivery.id == *id) {
            handover.unresolved.and_then(|end| end.reason)
        } else {
            None
        }
    }))
}

fn terminal_here(directory: &ConversationDir, turn: &str) -> io::Result<Option<Finished>> {
    optional(&terminal_path(directory, turn))
}

/// The first terminal outcome is authoritative. Later readers can be added
/// when acceptance was reported after completion, without changing the end.
pub fn finish(
    directory: &ConversationDir,
    turn: &str,
    end: TurnEnd,
    readers: &[Address],
) -> io::Result<(Finished, bool)> {
    finish_at(directory, turn, end, readers, None, false)
}

pub fn finish_at(
    directory: &ConversationDir,
    turn: &str,
    end: TurnEnd,
    readers: &[Address],
    ended_at: Option<Timestamp>,
    settles_queue: bool,
) -> io::Result<(Finished, bool)> {
    let _transaction = crate::resident_storage::lock();
    let prior = terminal_here(directory, turn)?;
    let first = prior.is_none();
    let mut finished = match prior {
        Some(finished) => finished,
        None => {
            let counter = directory.turns().join("order");
            let order = optional::<u64>(&counter)?
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| io::Error::other("terminal order exhausted"))?;
            // A stop after this reservation leaves a harmless gap, never a reused order.
            write(&counter, &order)?;
            Finished {
                turn: turn.to_string(),
                at: Timestamp::now(),
                ended_at,
                end,
                readers: Vec::new(),
                settles_queue,
                queue_order: order,
            }
        }
    };
    let before = finished.readers.clone();
    finished.readers.extend_from_slice(readers);
    finished.readers.sort();
    finished.readers.dedup();
    if first || finished.readers != before {
        write(&terminal_path(directory, turn), &finished)?;
    }
    Ok((finished, first))
}

pub fn terminals(directory: &ConversationDir) -> io::Result<Vec<Finished>> {
    let _transaction = crate::resident_storage::lock();
    list(&directory.turns())
}

pub fn remember_session(directory: &ConversationDir, turn: &str, session: &str) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let key = crate::speech::fixed_id("turn", &[turn]);
    let path = directory.turn_sessions().join(format!("{key}.json"));
    match create(&path, &session) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if read::<String>(&path)? != session {
                return Err(io::Error::other("turn belongs to a different conversation"));
            }
            Ok(())
        }
        result => result,
    }
}

pub fn session(directory: &ConversationDir, turn: &str) -> io::Result<Option<String>> {
    let _transaction = crate::resident_storage::lock();
    let key = crate::speech::fixed_id("turn", &[turn]);
    optional(&directory.turn_sessions().join(format!("{key}.json")))
}

/// Whether this event identity has already been durably published.
pub(crate) fn has_event(directory: &ConversationDir, key: &str) -> io::Result<bool> {
    let _transaction = crate::resident_storage::lock();
    let key = crate::speech::fixed_id("event", &[key]);
    directory.session_events().join(format!("{key}.json")).try_exists()
}

pub fn event(directory: &ConversationDir, key: &str, event: &SessionEvent) -> io::Result<bool> {
    let _transaction = crate::resident_storage::lock();
    let key = crate::speech::fixed_id("event", &[key]);
    crate::session_record::append(directory, key.as_str(), event)
}

pub fn history(directory: &ConversationDir) -> io::Result<Vec<SessionEvent>> {
    let _transaction = crate::resident_storage::lock();
    crate::session_record::history(directory)
}

pub(crate) fn handover_path(directory: &ConversationDir, id: &NoticeId) -> PathBuf {
    directory.handovers().join(format!("{id}.json"))
}

/// Whether the handover `id` was last written before `before`, any time
/// with none: a node recovering what the one before it left takes only
/// what that one wrote.
pub fn written_before(
    directory: &ConversationDir,
    id: &NoticeId,
    before: Option<std::time::SystemTime>,
) -> io::Result<bool> {
    let Some(before) = before else { return Ok(true) };
    match std::fs::metadata(handover_path(directory, id)) {
        Ok(metadata) => Ok(metadata.modified()? < before),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn terminal_path(directory: &ConversationDir, turn: &str) -> PathBuf {
    let id = crate::speech::fixed_id("turn", &[turn]);
    directory.turns().join(format!("{id}.json"))
}

fn list<T: serde::de::DeserializeOwned>(directory: &Path) -> io::Result<Vec<T>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut values = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "json")
            && !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            values.push(read(&path)?);
        }
    }
    Ok(values)
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))
}

fn optional<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match read(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    ensure_private_dir(path.parent().expect("a record has a directory"))?;
    publish(path, &serde_json::to_vec(value).map_err(io::Error::other)?, Permissions::Private)
}

fn create(path: &Path, value: &impl Serialize) -> io::Result<()> {
    ensure_private_dir(path.parent().expect("a record has a directory"))?;
    publish_new(path, &serde_json::to_vec(value).map_err(io::Error::other)?, Permissions::Private)
}

pub fn handovers(directory: &ConversationDir) -> io::Result<Vec<Handover>> {
    let _transaction = crate::resident_storage::lock();
    handovers_here(directory)
}
pub fn terminal(directory: &ConversationDir, turn: &str) -> io::Result<Option<Finished>> {
    let _transaction = crate::resident_storage::lock();
    terminal_here(directory, turn)
}
