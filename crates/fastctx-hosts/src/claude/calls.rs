//! Durable, per-call evidence shared by the mod and the MCP server.

use std::io;
use std::path::Path;

use serde::Deserialize;

use super::exchange::{self, Registration};

#[derive(Deserialize)]
struct Proof {
    tool_use_id: String,
    registration: Registration,
}

/// Finds exactly one complete report of this call. Reports survive event
/// consumption and node restarts; duplicate ids across sessions are ambiguous.
pub fn registration(world: &Path, id: &str) -> io::Result<Registration> {
    if id.is_empty() {
        return Err(io::Error::other("the Claude tool use id is empty"));
    }
    let name: String = id.encode_utf16().map(|unit| format!("{unit:04x}")).collect();
    let mut found = None;
    for entry in std::fs::read_dir(exchange::sessions(world))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("calls").join(&name).with_extension("json");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !path.with_extension("done").try_exists()? {
            return Err(io::Error::other(format!("Claude call {id} has an incomplete mod report")));
        }
        let proof: Proof = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if proof.tool_use_id != id || entry.file_name().to_str() != Some(&proof.registration.session) {
            return Err(io::Error::other(format!("Claude call {id} has inconsistent mod evidence")));
        }
        if found.replace(proof.registration).is_some() {
            return Err(io::Error::other(format!("Claude call {id} was reported by more than one session")));
        }
    }
    found.ok_or_else(|| {
        io::Error::other(format!("the World's mod has not reported Claude call {id}; its session is unknown"))
    })
}
