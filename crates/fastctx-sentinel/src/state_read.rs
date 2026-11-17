//! Read-only recovery when the published state is missing.

use crate::state::State;
use std::{io, path::Path};

pub(crate) fn recover(directory: &Path) -> Result<Option<State>, String> {
    // No sequence frontier or history index survives a missing state.json.
    // Preserve the old first response by discovering the latest completed
    // snapshot; this exceptional discovery is still unbounded (2026-11-11).
    let history = directory.join("history");
    let entries = match fastctx_platform::disk::read_dir(&history) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut latest = None;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let sequence = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u64>().ok())
            .ok_or("invalid sentinel history filename")?;
        if latest.as_ref().is_none_or(|(held, _)| sequence > *held) {
            latest = Some((sequence, path));
        }
    }
    let Some((sequence, path)) = latest else { return Ok(None) };
    let recovered: State =
        crate::bounded::load(&path, crate::bounded::STATE_BYTES)?.ok_or("sentinel history disappeared")?;
    if recovered.sequence != sequence || recovered.pending.is_some() {
        return Err("invalid completed sentinel snapshot".into());
    }
    Ok(Some(recovered))
}
