//! The records a hub directory holds, as a move copies them: every file and
//! directory but the hub's identity and what belongs to the one machine it
//! runs on (`local`), each file by its size and SHA-256.
//!
//! Both ends list what they hold the same way, so the old hub sends only
//! what differs, and the new place takes over only when the digest of what
//! it holds is the digest of what the old hub held when it stopped.

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use fastctx_net::sign::Digest;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

/// One file or directory of the records, by its path under the hub's
/// directory, with `/` between names.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub path: String,
    /// `None` for a directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Sum>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Sum {
    pub bytes: u64,
    pub sha256: Digest,
}

/// Whether the top-level name `name` is the hub's own on the machine it
/// runs on, never copied nor replaced by a move: its identity and settings
/// (`hub.json`, `tls`, `login.secret`), which the new place makes from what
/// it is given, the records of the move itself, its log and how it was
/// doing, the releases put there for this machine, and the letters set
/// aside to be deleted.
pub(crate) fn local(name: &str) -> bool {
    matches!(
        name,
        "hub.json"
            | crate::store::shelf::WAL
            | crate::store::shelf::SHM
            | crate::store::shelf::JOURNAL
            | "tls"
            | "login.secret"
            | "releases"
            | crate::health::FILE
            | crate::store::TRASH
            | super::ARRIVING
            | super::OUTGOING
            | super::RETIRED
    ) || name.starts_with("hub.log")
}

/// The records under `dir`, sorted by path. A name starting with a dot is
/// a write in progress, never a record.
pub(crate) fn manifest(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    walk(dir, "", &mut entries, false)?;
    entries.sort();
    Ok(entries)
}

pub(super) fn snapshot(dir: &Path) -> io::Result<Vec<Entry>> {
    super::snapshot::capture(dir)?;
    let mut entries = Vec::new();
    walk(dir, "", &mut entries, true)?;
    entries.push(Entry { path: crate::store::shelf::FILE.into(), file: Some(sum(&super::snapshot::path(dir))?) });
    entries.sort();
    Ok(entries)
}

fn walk(dir: &Path, prefix: &str, entries: &mut Vec<Entry>, snapshot: bool) -> io::Result<()> {
    for found in std::fs::read_dir(dir)? {
        let found = found?;
        let name = found.file_name();
        let name = name.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} has a name that is not UTF-8", found.path().display()),
            )
        })?;
        if name.starts_with('.')
            || (prefix.is_empty() && (local(name) || snapshot && name == crate::store::shelf::FILE))
        {
            continue;
        }
        let path = if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
        let kind = found.file_type()?;
        if kind.is_dir() {
            entries.push(Entry { path: path.clone(), file: None });
            walk(&found.path(), &path, entries, snapshot)?;
        } else if kind.is_file() {
            entries.push(Entry { path, file: Some(sum(&found.path())?) });
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is neither a file nor a directory", found.path().display()),
            ));
        }
    }
    Ok(())
}

fn sum(path: &Path) -> io::Result<Sum> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut bytes = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes += read as u64;
    }
    Ok(Sum { bytes, sha256: Digest(hasher.finalize().into()) })
}

/// The digest of a whole listing, as both ends compute it.
pub(crate) fn digest(entries: &[Entry]) -> Digest {
    let mut hasher = sha2::Sha256::new();
    for entry in entries {
        hasher.update(entry.path.as_bytes());
        hasher.update([0]);
        match &entry.file {
            Some(sum) => {
                hasher.update(sum.bytes.to_le_bytes());
                hasher.update(sum.sha256.0);
            }
            None => hasher.update(b"/"),
        }
        hasher.update([b'\n']);
    }
    Digest(hasher.finalize().into())
}

/// Where the record at `path`, as the other end wrote it, is under `dir`;
/// refused when it is not a record's path.
pub(crate) fn place(dir: &Path, path: &str) -> Result<PathBuf, String> {
    let mut place = dir.to_path_buf();
    for (index, name) in path.split('/').enumerate() {
        let odd = name.is_empty()
            || name == "."
            || name == ".."
            || name.starts_with('.')
            || name.contains(['\\', ':', '\0'])
            || (index == 0 && local(name));
        if odd {
            return Err(format!("{path:?} is not the path of a hub's record"));
        }
        place.push(name);
    }
    Ok(place)
}
