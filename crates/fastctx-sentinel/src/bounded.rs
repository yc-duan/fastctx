//! Bounded reads of private sentinel snapshots and completed runs.

use serde::de::DeserializeOwned;
use std::path::Path;

pub(crate) const STATE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const RUN_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn load<T: DeserializeOwned>(path: &Path, limit: usize) -> Result<Option<T>, String> {
    load_counted(path, limit).map(|loaded| loaded.map(|(value, _)| value))
}

pub(crate) fn load_counted<T: DeserializeOwned>(path: &Path, limit: usize) -> Result<Option<(T, usize)>, String> {
    let bytes = match fastctx_platform::disk::read_bounded(path, limit) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map(|value| Some((value, bytes.len())))
        .map_err(|error| format!("{}: {error}", path.display()))
}
