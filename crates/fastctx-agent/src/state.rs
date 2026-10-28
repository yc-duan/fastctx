//! Atomic snapshots; callers serialize writers to each path.
use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;

pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    fastctx_platform::publish(path, &bytes, fastctx_platform::Permissions::Private)
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}
