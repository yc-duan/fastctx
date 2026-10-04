//! Active World calls, indexed by the host's tool-use id. The node is the
//! only writer; each proof lives as long as the mod's per-call helper.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, ensure_private_dir, is_alive, publish_new};
use fastctx_types::ProcessIdentity;
use serde::{Deserialize, Serialize};

use super::exchange::{self, Registration};

#[derive(Clone, Deserialize, Serialize)]
pub struct Proof {
    pub tool_use_id: String,
    pub registration: Registration,
    pub owner: ProcessIdentity,
}

pub fn directory(world: &Path) -> PathBuf {
    world.join("claude-calls")
}

fn path(world: &Path, id: &str) -> io::Result<PathBuf> {
    if id.is_empty() {
        return Err(io::Error::other("the Claude tool use id is empty"));
    }
    let name: String = id.encode_utf16().map(|unit| format!("{unit:04x}")).collect();
    Ok(directory(world).join(name).with_extension("json"))
}

pub fn read(world: &Path, id: &str) -> io::Result<Option<Proof>> {
    let bytes = match std::fs::read(path(world, id)?) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let proof: Proof = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if proof.tool_use_id != id {
        return Err(io::Error::other("the Claude call index contains a different tool-use id"));
    }
    Ok(Some(proof))
}

/// Publishes the whole proof atomically. A second live claimant is refused
/// before its tool runs, rather than overwriting the first call's identity.
/// The caller serializes this with `finish` under the node's calls lock.
pub fn begin(world: &Path, proof: &Proof) -> io::Result<()> {
    ensure_private_dir(&directory(world))?;
    if !is_alive(&proof.owner)? {
        return Err(io::Error::other("the Claude call's helper has already exited"));
    }
    if let Some(previous) = read(world, &proof.tool_use_id)? {
        if is_alive(&previous.owner)? {
            return Err(io::Error::other(format!("Claude call {} already has a live claimant", proof.tool_use_id)));
        }
        finish(world, &proof.tool_use_id, &previous.owner)?;
    }
    let bytes = serde_json::to_vec(proof).map_err(io::Error::other)?;
    publish_new(&path(world, &proof.tool_use_id)?, &bytes, Permissions::Private)
}

/// Reads one indexed proof, without visiting any session directories.
pub fn registration(world: &Path, id: &str) -> io::Result<Registration> {
    let proof = read(world, id)?.ok_or_else(|| {
        io::Error::other(format!("the World's mod has no active proof for Claude call {id}; its session is unknown"))
    })?;
    if !is_alive(&proof.owner)? {
        return Err(io::Error::other(format!("Claude call {id}'s helper has exited; its proof is no longer active")));
    }
    Ok(proof.registration)
}

/// Removes only this helper's claim. A late cleanup cannot remove a later
/// call that reused the id. Missing proofs have already been released.
pub fn finish(world: &Path, id: &str, owner: &ProcessIdentity) -> io::Result<bool> {
    let Some(proof) = read(world, id)? else { return Ok(true) };
    if &proof.owner != owner {
        return Ok(false);
    }
    std::fs::remove_file(path(world, id)?)?;
    Ok(true)
}

/// Startup recovery visits only outstanding calls, never session history.
pub fn outstanding(world: &Path) -> io::Result<Vec<io::Result<Proof>>> {
    ensure_private_dir(&directory(world))?;
    let mut proofs = Vec::new();
    for entry in std::fs::read_dir(directory(world))? {
        let entry = entry?;
        // Atomic publication can leave its staging file if the node dies
        // before renaming it. Recovery runs before this sole writer serves.
        let name = entry.file_name();
        let temporary = name
            .to_str()
            .and_then(|name| name.strip_prefix(".fastctx-"))
            .and_then(|name| name.strip_suffix(".tmp"))
            .and_then(|name| name.split_once('-'))
            .is_some_and(|(pid, sequence)| {
                !pid.is_empty()
                    && !sequence.is_empty()
                    && pid.bytes().all(|byte| byte.is_ascii_digit())
                    && sequence.bytes().all(|byte| byte.is_ascii_digit())
            });
        if temporary && entry.file_type()?.is_file() {
            std::fs::remove_file(entry.path())?;
            continue;
        }
        if entry.path().extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let read = (|| {
            let proof: Proof = serde_json::from_slice(&std::fs::read(entry.path())?).map_err(io::Error::other)?;
            if entry.path() != path(world, &proof.tool_use_id)? {
                return Err(io::Error::other("a Claude call proof is stored under a different id"));
            }
            Ok(proof)
        })();
        proofs.push(read.map_err(|error: io::Error| io::Error::other(format!("{}: {error}", entry.path().display()))));
    }
    Ok(proofs)
}

/// Retires the previous per-session proof format once at node startup.
/// That protocol's servers are refused by this node; none of these files
/// can establish a current call's identity. Other session records stay put.
pub fn retire_legacy(world: &Path) -> io::Result<usize> {
    let sessions = match std::fs::read_dir(exchange::sessions(world)) {
        Ok(sessions) => sessions,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let root = world.canonicalize()?;
    let mut removed = 0;
    for session in sessions {
        let session = session?;
        if !session.file_type()?.is_dir() {
            continue;
        }
        let directory = session.path().join("calls");
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !directory.canonicalize()?.starts_with(&root) {
            return Err(io::Error::other(format!("legacy call directory {} leaves this World", directory.display())));
        }
        let mut empty = true;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
            // The old encoder wrote four lowercase hex digits per UTF-16
            // unit, followed only by .json or its empty .done marker.
            let owned = !stem.is_empty()
                && stem.len().is_multiple_of(4)
                && stem.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                && path.extension().is_some_and(|extension| extension == "json" || extension == "done")
                && entry.file_type()?.is_file();
            if owned {
                std::fs::remove_file(path)?;
                removed += 1;
            } else {
                empty = false;
            }
        }
        if empty {
            std::fs::remove_dir(directory)?;
        }
    }
    Ok(removed)
}
