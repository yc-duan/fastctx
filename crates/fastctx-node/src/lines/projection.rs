//! Captured-note indexes have the same lifetime and byte boundary as their line source.

use super::State;
use fastctx_hands::run::projection::CapturedNotes;
use fastctx_types::run_facts::WriteFact;
use sha2::{Digest as _, Sha256};
use std::io;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

type Revision = Option<(u64, SystemTime)>;
type Opened = (Option<CapturedNotes>, Vec<WriteFact>, Revision);

pub(super) fn verify(projection: Option<&CapturedNotes>, expected: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected else { return Ok(()) };
    let notes = projection.map(CapturedNotes::matches).unwrap_or_default();
    let bytes = serde_json::to_vec(&notes).map_err(|error| error.to_string())?;
    if format!("{:x}", Sha256::digest(bytes)) != expected {
        return Err("the output annotations changed since its history was read".into());
    }
    Ok(())
}

pub(super) fn revision(record: Option<&Path>) -> Result<Revision, String> {
    let Some(record) = record else { return Ok(None) };
    match std::fs::metadata(record.with_extension("writes.jsonl")) {
        Ok(metadata) => Ok(Some((metadata.len(), metadata.modified().map_err(|error| error.to_string())?))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn open(path: &Path, bytes: u64, record: Option<&Path>, cancelled: &AtomicBool) -> Result<Opened, String> {
    let Some(record) = record else { return Ok((None, Vec::new(), None)) };
    let revision = revision(Some(record))?;
    let writes = crate::run_facts::writes(record).map_err(|error| error.to_string())?;
    let projection =
        CapturedNotes::read_cancellable(path, bytes, &writes, Some(cancelled)).map_err(|error| error.to_string())?;
    Ok((Some(projection), writes, revision))
}

/// Brings the notes up to `bytes` of the output; whether what they mark
/// changed. A refresh that fails part way leaves the occurrences it was
/// adding half written: the notes are then read afresh at the next look,
/// and until then show what they showed.
pub(super) fn refresh(
    state: &mut State,
    path: &Path,
    bytes: u64,
    record: Option<&Path>,
    revision: Revision,
    cancelled: &AtomicBool,
) -> io::Result<bool> {
    let Some(record) = record else { return Ok(false) };
    if state.writes_revision != revision || state.notes_broken {
        state.writes = crate::run_facts::writes(record)?;
        state.writes_revision = revision;
    }
    if state.notes_broken {
        state.projection = Some(CapturedNotes::read_cancellable(path, bytes, &state.writes, Some(cancelled))?);
        state.notes_broken = false;
        return Ok(true);
    }
    let refreshed = state
        .projection
        .as_mut()
        .ok_or_else(|| io::Error::other("output projection is closed"))?
        .refresh_cancellable(path, bytes, &state.writes, Some(cancelled));
    state.notes_broken = refreshed.is_err();
    refreshed
}
