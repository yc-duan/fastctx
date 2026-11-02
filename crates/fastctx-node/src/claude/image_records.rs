//! Only transcripts whose mod registration belongs to this resident.

use fastctx_hosts::claude::exchange;
use fastctx_types::{ResidentId, ipc::Failure};
use std::path::PathBuf;

pub(crate) fn paths(node: &crate::Node, resident: &ResidentId) -> Result<Vec<PathBuf>, Failure> {
    let root = exchange::sessions(node.world.path());
    let entries = match fastctx_platform::disk::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Failure::new(error.to_string())),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let directory = entry.map_err(|error| Failure::new(error.to_string()))?.path();
        if !fastctx_platform::disk::is_dir(&directory) {
            continue;
        }
        let bytes = match fastctx_platform::disk::read(directory.join("resident.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(Failure::new(error.to_string())),
        };
        let owner: ResidentId = serde_json::from_slice(&bytes).map_err(|error| Failure::new(error.to_string()))?;
        if owner != *resident {
            continue;
        }
        if let Some(path) = exchange::transcript(&directory).map_err(|error| Failure::new(error.to_string()))? {
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}
