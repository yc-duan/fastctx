//! The texts a copy names: its hub revision and its waiting edits, each kept
//! once under the copy's folder of bodies, named by its hash, and checked
//! against that hash whenever it is read.

use super::state::Copy;
use fastctx_types::layout::WorldDir;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io, path::PathBuf};

/// A text by its SHA-256 and length; the empty text is never stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Text {
    pub hash: String,
    pub len: u64,
}

impl Default for Text {
    fn default() -> Self {
        Self::of("")
    }
}

impl Text {
    pub(crate) fn of(text: &str) -> Self {
        Self::of_bytes(text.as_bytes())
    }
    pub(crate) fn of_bytes(bytes: &[u8]) -> Self {
        Self { hash: format!("{:x}", Sha256::digest(bytes)), len: bytes.len() as u64 }
    }
}

fn folder(world: &WorldDir, id: &str) -> PathBuf {
    super::state::root(world).join("bodies").join(id)
}

/// Keeps `text` for the copy `id`, unless it is kept already.
pub(crate) fn put(world: &WorldDir, id: &str, text: &str) -> io::Result<Text> {
    let named = Text::of(text);
    if text.is_empty() {
        return Ok(named);
    }
    let directory = folder(world, id);
    let path = directory.join(&named.hash);
    if fastctx_platform::disk::metadata(&path).is_ok_and(|metadata| metadata.len() == named.len) {
        return Ok(named);
    }
    fastctx_platform::ensure_private_dir(&directory)?;
    if let Some(warning) = super::storage::publish(&path, text.as_bytes())? {
        eprintln!("fastctx: {warning}");
    }
    Ok(named)
}

/// The text the copy `id` names. A file that does not hold it is an error,
/// never another text.
pub(crate) fn read(world: &WorldDir, id: &str, text: &Text) -> io::Result<String> {
    if text.len == 0 {
        return Ok(String::new());
    }
    let body = read_hash(world, id, &text.hash)?;
    if body.len() as u64 != text.len {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a kept replica text does not match its length"));
    }
    Ok(body)
}

/// Reads a retained observation, whose record names its hash alone.
pub(crate) fn read_hash(world: &WorldDir, id: &str, hash: &str) -> io::Result<String> {
    if hash == Text::of("").hash {
        return Ok(String::new());
    }
    let bytes = fastctx_platform::disk::read(folder(world, id).join(hash))?;
    if format!("{:x}", Sha256::digest(&bytes)) != hash {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a kept replica text does not match its hash"));
    }
    String::from_utf8(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a kept replica text is not UTF-8"))
}

/// Lets go of the texts `copy` no longer names.
pub(crate) fn tidy(world: &WorldDir, copy: &Copy) -> io::Result<()> {
    let mut named: BTreeSet<&str> = BTreeSet::new();
    named.insert(&copy.body.hash);
    named.extend(copy.observed.values().filter_map(|observed| observed.hash.as_deref()));
    for pending in &copy.pending {
        named.insert(&pending.before.hash);
        named.insert(&pending.text.hash);
    }
    let entries = match fastctx_platform::disk::read_dir(folder(world, &copy.id())) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !named.contains(entry.file_name().to_string_lossy().as_ref()) {
            match fastctx_platform::disk::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

/// Removes every text of the copy `id`.
pub(crate) fn remove(world: &WorldDir, id: &str) -> io::Result<()> {
    super::removal::delete(world, &folder(world, id))
}
