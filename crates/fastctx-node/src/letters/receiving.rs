//! Durable fragments stay until the whole letter's business accepts it. A
//! completion marker precedes deleting pieces, so replaying the final fragment
//! after a crash cannot leave an already accepted letter waiting for lost pieces.

mod capacity;
mod failed;

use std::collections::HashSet;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use super::Identity;
use fastctx_net::reassembly::{Buffer, Reservation};
use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::remote::Letter;

use crate::Node;

pub(super) const MAX_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_PARTS: usize = 4096;
const LEGACY_BYTES: usize = 64 * 1024 * 1024;
const PART_BYTES: usize = fastctx_net::wire::MAX_CONTROL_FRAME;
// A JSON string can encode each input byte as six bytes, plus tuple overhead.
const STORED_BYTES: usize = PART_BYTES * 6 + 64;

static ACTIVE: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);

struct Active(PathBuf);

impl Active {
    fn take(path: PathBuf) -> io::Result<Self> {
        if !ACTIVE.lock().expect("never poisoned").insert(path.clone()) {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "this letter is already being received"));
        }
        Ok(Self(path))
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().expect("never poisoned");
        active.remove(&self.0);
        if active.is_empty() {
            *active = HashSet::new();
        }
    }
}

/// Held through the business handoff; failed or cancelled handoffs keep every
/// durable piece. Only `finish` records acceptance and deletes them.
pub(crate) struct Completion {
    active: Active,
    total: usize,
    _memory: Reservation,
}

impl Completion {
    pub(crate) fn finish(self) -> io::Result<()> {
        let path = self.active.0.join("done");
        let bytes = self.total.to_string();
        publish(&path, bytes.as_bytes(), Permissions::Private)?;
        delete_parts(&self.active.0, self.total)
    }
}

/// Keeps a fragment, returning the complete letter and its handoff guard when
/// all pieces exist. Inner errors reject malformed input, stored or arriving;
/// outer errors leave delivery pending so a disk failure or a full memory
/// budget can be retried. Pieces are read only once all of them are kept.
pub(crate) fn receive(
    node: &Node,
    identity: &Identity,
    index: usize,
    total: usize,
    text: &str,
) -> io::Result<Result<Option<(Letter, Completion)>, String>> {
    if total == 0 || index >= total {
        return Ok(Err("invalid letter fragment numbering".to_string()));
    }
    if text.is_empty() || text.len() > PART_BYTES {
        return Ok(Err(format!("a letter fragment must contain 1 to {PART_BYTES} bytes")));
    }
    keep(node.world.path().join("letter-parts"), identity, index, total, text)
}

/// Keeps piece `index` of `total` in `directory`, which holds that letter's.
fn keep(
    root: PathBuf,
    identity: &Identity,
    index: usize,
    total: usize,
    text: &str,
) -> io::Result<Result<Option<(Letter, Completion)>, String>> {
    let directory = root.join(identity.id.as_str());
    let active = Active::take(directory.clone())?;
    if let Err(error) = identity.migrate_fragments(&root, STORED_BYTES) {
        if error.kind() == io::ErrorKind::InvalidData {
            return Ok(Err(error.to_string()));
        }
        return Err(error);
    }
    ensure_private_dir(&directory)?;
    match read_bounded(&directory.join("done"), 32)? {
        Some(Ok(done)) if done == total.to_string().as_bytes() => {
            // A prior publication may have become visible before its durability
            // step failed. Recommit the marker before deleting any acknowledged part.
            publish(&directory.join("done"), &done, Permissions::Private)?;
            delete_parts(&directory, total)?;
            return Ok(Ok(None));
        }
        Some(_) => return Ok(Err("letter fragments disagree about their total".to_string())),
        None => {}
    }
    if let Err(reason) = capacity::admit(&directory, total)? {
        return Ok(Err(reason));
    }
    // Which pieces are kept, by their numbers. Old versions kept the same
    // numbered tuples with no manifest, so they are taken where they are.
    let (mut kept, mut current) = (0_usize, false);
    for entry in std::fs::read_dir(&directory)? {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(part) = name.parse::<usize>() else { continue };
        if part >= total || name != part.to_string() {
            return Ok(Err("letter fragments disagree about their total or numbering".to_string()));
        }
        kept += 1;
        current |= part == index;
    }
    if current {
        match stored(&directory, index, total)? {
            Ok(piece) if piece == text => {}
            Ok(_) => return Ok(Err("a letter fragment changed under the same id".to_string())),
            Err(reason) => return Ok(Err(reason)),
        }
    }
    // Recommit a matching duplicate too: seeing bytes from a failed publication
    // does not establish that its directory entry was made durable.
    let bytes = serde_json::to_vec(&(total, text)).map_err(io::Error::other)?;
    publish(&directory.join(index.to_string()), &bytes, Permissions::Private)?;
    drop(bytes);
    if kept + usize::from(!current) < total {
        return Ok(Ok(None));
    }
    // Every piece is kept: the whole is put together within the process's
    // budget, held until its business is done with it.
    let capacity = total.saturating_mul(PART_BYTES).min(LEGACY_BYTES);
    let mut whole = Buffer::new(capacity, STORED_BYTES + PART_BYTES).map_err(io::Error::other)?;
    for part in 0..total {
        let other;
        let piece = if part == index {
            text
        } else {
            other = match stored(&directory, part, total)? {
                Ok(piece) => piece,
                Err(reason) => return Ok(Err(reason)),
            };
            other.as_str()
        };
        if piece.len() > LEGACY_BYTES - whole.len() {
            drop(whole);
            let reason = failed::keep(&directory, identity, total, LEGACY_BYTES)?;
            return Ok(Err(reason));
        }
        whole.extend(piece.as_bytes()).map_err(io::Error::other)?;
    }
    let letter = match serde_json::from_slice::<Letter>(&whole) {
        Ok(letter) => letter,
        Err(error) => return Ok(Err(format!("the fragmented letter cannot be read: {error}"))),
    };
    if matches!(letter, Letter::Fragment { .. }) {
        return Ok(Err("nested letter fragments".to_string()));
    }
    Ok(Ok(Some((letter, Completion { active, total, _memory: whole.into_reservation() }))))
}

/// The piece kept as `part` of `total`, or why it cannot be one.
fn stored(directory: &Path, part: usize, total: usize) -> io::Result<Result<String, String>> {
    let bytes = match read_bounded(&directory.join(part.to_string()), STORED_BYTES)? {
        Some(Ok(bytes)) => bytes,
        Some(Err(reason)) => return Ok(Err(reason)),
        None => return Err(io::Error::new(io::ErrorKind::NotFound, format!("letter fragment {part} went missing"))),
    };
    match serde_json::from_slice::<(usize, String)>(&bytes) {
        Ok((count, piece)) if count == total && !piece.is_empty() && piece.len() <= PART_BYTES => Ok(Ok(piece)),
        Ok(_) => Ok(Err("stored letter fragments disagree or exceed the fragment limit".to_string())),
        Err(error) => Ok(Err(format!("a stored letter fragment cannot be read: {error}"))),
    }
}

/// The file at `path` if there is one, or why it is too large to be what it
/// should: a refusal, not a failure of the disk to retry for ever.
fn read_bounded(path: &Path, limit: usize) -> io::Result<Option<Result<Vec<u8>, String>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Ok(Some(Err(format!("{} exceeds {limit} bytes", path.display()))));
    }
    Ok(Some(Ok(bytes)))
}

fn delete_parts(directory: &Path, total: usize) -> io::Result<()> {
    for part in 0..total {
        match std::fs::remove_file(directory.join(part.to_string())) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
