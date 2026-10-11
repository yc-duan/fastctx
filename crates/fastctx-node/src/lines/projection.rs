//! Captured-note indexes have the same lifetime and byte boundary as their line source.

use super::State;
use fastctx_hands::run::projection::CapturedNotes;
use fastctx_types::run_facts::WriteFact;
use std::path::Path;
use std::time::SystemTime;

type Revision = Option<(u64, SystemTime)>;
type Opened = (Option<CapturedNotes>, Vec<WriteFact>, Revision);

pub(super) fn revision(record: Option<&Path>) -> Result<Revision, String> {
    let Some(record) = record else { return Ok(None) };
    match std::fs::metadata(record.with_extension("writes.jsonl")) {
        Ok(metadata) => Ok(Some((metadata.len(), metadata.modified().map_err(|error| error.to_string())?))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn open(path: &Path, bytes: u64, record: Option<&Path>) -> Result<Opened, String> {
    let Some(record) = record else { return Ok((None, Vec::new(), None)) };
    let revision = revision(Some(record))?;
    let writes = crate::run_facts::writes(record).map_err(|error| error.to_string())?;
    let projection = CapturedNotes::read(path, bytes, &writes).map_err(|error| error.to_string())?;
    Ok((Some(projection), writes, revision))
}

pub(super) fn refresh(
    state: &mut State,
    path: &Path,
    bytes: u64,
    record: Option<&Path>,
    revision: Revision,
) -> Result<bool, String> {
    let Some(record) = record else { return Ok(false) };
    if state.writes_revision != revision {
        state.writes = crate::run_facts::writes(record).map_err(|error| error.to_string())?;
        state.writes_revision = revision;
    }
    state
        .projection
        .as_mut()
        .ok_or_else(|| "output projection is closed".to_string())?
        .refresh(path, bytes, &state.writes)
        .map_err(|error| error.to_string())
}
