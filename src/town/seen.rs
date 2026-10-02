//! What a resident has seen of a file, so a whole-file `write` can refuse to wipe out changes
//! made since (`specs/world-write.md`, "整份覆盖前多一道检查").
//!
//! `fastctx view` appends one line per file it showed to `seen` in the resident's directory on
//! the machine where it ran; the write tools on that machine read the latest line for a path.
//! A file shown whole is remembered by its content hash. A file shown in part is remembered by
//! its size and modification time, since hashing a multi-gigabyte log on every partial view
//! would cost more than the view.

use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) path: PathBuf,
    /// SHA-256 of the bytes, when the whole file was shown.
    #[serde(default)]
    pub(crate) hash: Option<String>,
    pub(crate) size: u64,
    /// Modification time in nanoseconds since the Unix epoch, as the file system reported it.
    pub(crate) modified: Option<u128>,
    /// The encoding the text was decoded with, when it was shown as text. A file whose bytes
    /// fit several legacy encodings is written back in the one the resident read it in.
    #[serde(default)]
    pub(crate) encoding: Option<String>,
}

impl Seen {
    /// What a file looks like now, measured the same way as when it was seen.
    pub(crate) fn changed_since(&self) -> Result<bool, String> {
        let metadata = match std::fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(format!("Cannot inspect {}: {error}", crate::paths::display_path(&self.path))),
        };
        match &self.hash {
            Some(hash) => {
                let bytes = std::fs::read(&self.path)
                    .map_err(|error| format!("Cannot read {}: {error}", crate::paths::display_path(&self.path)))?;
                Ok(&hex::encode(crate::world::crypto::sha256(&bytes)) != hash)
            }
            None => Ok(metadata.len() != self.size || modified(&metadata) != self.modified),
        }
    }
}

/// The record of a file as it is now; `hash` when its whole content was shown.
pub(crate) fn observe(path: &Path, whole: Option<&[u8]>, encoding: Option<&str>) -> Result<Seen, String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("Cannot inspect {}: {error}", crate::paths::display_path(path)))?;
    Ok(Seen {
        path: path.to_path_buf(),
        hash: whole.map(|bytes| hex::encode(crate::world::crypto::sha256(bytes))),
        size: metadata.len(),
        modified: modified(&metadata),
        encoding: encoding.map(str::to_string),
    })
}

/// Appends what was seen to the resident's `seen` file.
pub(crate) fn record(resident_dir: &Path, seen: &[Seen]) -> Result<(), String> {
    if seen.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(resident_dir).map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(resident_dir)))?;
    let path = resident_dir.join("seen");
    let mut lines = String::new();
    for entry in seen {
        lines.push_str(&serde_json::to_string(entry).map_err(|error| format!("Cannot encode a seen line: {error}"))?);
        lines.push('\n');
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(lines.as_bytes()))
        .map_err(|error| format!("Cannot append to {}: {error}", crate::paths::display_path(&path)))
}

/// The latest record of `path` in a resident's `seen` file.
pub(crate) fn latest(resident_dir: &Path, path: &Path) -> Option<Seen> {
    let text = std::fs::read_to_string(resident_dir.join("seen")).ok()?;
    text.lines().rev().filter_map(|line| serde_json::from_str::<Seen>(line).ok()).find(|seen| same_path(&seen.path, path))
}

fn same_path(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy().replace('\\', "/").eq_ignore_ascii_case(&right.to_string_lossy().replace('\\', "/"))
    } else {
        left == right
    }
}

fn modified(metadata: &std::fs::Metadata) -> Option<u128> {
    metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|elapsed| elapsed.as_nanos())
}
