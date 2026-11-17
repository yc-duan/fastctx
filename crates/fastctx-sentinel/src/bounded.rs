//! Bounded reads of private sentinel snapshots and completed runs.

use serde::de::DeserializeOwned;
use std::path::Path;

pub(crate) const STATE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const RUN_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct LoadError {
    pub message: String,
    pub bytes: usize,
}

pub(crate) fn load<T: DeserializeOwned>(path: &Path, limit: usize) -> Result<Option<T>, String> {
    load_counted(path, limit).map(|loaded| loaded.map(|(value, _)| value)).map_err(|error| error.message)
}

pub(crate) fn load_counted<T: DeserializeOwned>(path: &Path, limit: usize) -> Result<Option<(T, usize)>, LoadError> {
    let bytes = match fastctx_platform::disk::read_bounded(path, limit) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            let bytes = match error.kind() {
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::InvalidInput => 0,
                _ => limit.saturating_add(1),
            };
            return Err(LoadError { message: format!("{}: {error}", path.display()), bytes });
        }
    };
    serde_json::from_slice(&bytes)
        .map(|value| Some((value, bytes.len())))
        .map_err(|error| LoadError { message: format!("{}: {error}", path.display()), bytes: bytes.len() })
}
