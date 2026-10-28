//! Which Claude session made a call to the World's tools, by the host's
//! tool-use id. The mod writes one proof before letting a World call
//! through; the node takes it when the call arrives. A proof whose call
//! never arrived, as when the call was cancelled or denied first, is swept
//! once its session is over.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::exchange::Registration;

#[derive(Clone, Deserialize, Serialize)]
pub struct Proof {
    pub tool_use_id: String,
    pub registration: Registration,
    /// The session's subagent that made the call; none for the session's
    /// own agent, and from an earlier mod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

pub fn directory(world: &Path) -> PathBuf {
    world.join("claude-calls")
}

/// Four hex digits per UTF-16 unit, as the mod spells it: any id makes one
/// plain file name.
fn path(world: &Path, id: &str) -> io::Result<PathBuf> {
    if id.is_empty() {
        return Err(io::Error::other("the Claude tool use id is empty"));
    }
    let name: String = id.encode_utf16().map(|unit| format!("{unit:04x}")).collect();
    Ok(directory(world).join(name).with_extension("json"))
}

/// The proof of call `id`, naming the session that made it, removed. The
/// mod finished writing it before the call left the host, so a proof read
/// here is whole.
pub fn take(world: &Path, id: &str) -> io::Result<Proof> {
    take_from(world, id, std::iter::empty::<PathBuf>())
}

/// Looks in the known sessions before the older shared proof directory.
pub fn take_from(world: &Path, id: &str, sessions: impl IntoIterator<Item = PathBuf>) -> io::Result<Proof> {
    let legacy = path(world, id)?;
    let mut paths: Vec<_> = sessions
        .into_iter()
        .map(|session| session.join("calls").join(legacy.file_name().expect("proof name")))
        .collect();
    paths.push(legacy);
    for path in paths {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(error),
        };
        let proof: Proof = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if proof.tool_use_id != id {
            return Err(io::Error::other(format!("the proof for Claude call {id} names another call")));
        }
        remove(&path)?;
        return Ok(proof);
    }
    Err(io::Error::other(format!(
        "the World's mod left no proof for Claude call {id}; the session that made it is unknown"
    )))
}

/// Removes the proofs whose session `over` says has ended. A proof that
/// cannot be read stays, and is said: its session cannot be told.
pub fn sweep(world: &Path, over: impl Fn(&Registration) -> io::Result<bool>) -> io::Result<Vec<String>> {
    let mut directories = vec![directory(world)];
    match std::fs::read_dir(super::exchange::sessions(world)) {
        Ok(sessions) => {
            for session in sessions {
                directories.push(session?.path().join("calls"));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut problems = Vec::new();
    for directory in directories {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let ended = std::fs::read(&path)
                .and_then(|bytes| serde_json::from_slice::<Proof>(&bytes).map_err(io::Error::other))
                .and_then(|proof| over(&proof.registration));
            match ended {
                Ok(true) => remove(&path)?,
                Ok(false) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => problems.push(format!("{}: {error}", path.display())),
            }
        }
    }
    Ok(problems)
}

/// Taken by its call or swept already is the same as removed.
fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
