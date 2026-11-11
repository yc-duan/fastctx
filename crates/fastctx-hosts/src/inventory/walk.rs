//! A walk of everything below a folder by the names of its directories'
//! entries, looking for directories that look like the agents' logins.
//!
//! Only names and entry types are read, as listing a directory gives them;
//! no file is opened. Links of every kind are not followed: Windows
//! junctions, symbolic links and the reparse points of cloud folders, whose
//! listing may fetch from the network, and on Linux symbolic links and the
//! mounts of other filesystems, each walked only when chosen itself.
//! A directory that looks like a login is walked into like any other: a
//! stray file in a folder (`auth.json` on a desktop) must not hide what is
//! below it. The walk keeps only the directories still to visit, so its
//! memory follows the shape of the tree, not its size.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::traces::{self, AGENTS, Entry};

/// How far a walk has got, shared with whoever watches it.
#[derive(Debug, Default)]
pub struct Counters {
    pub directories: AtomicU64,
    pub unreadable: AtomicU64,
    pub done: AtomicBool,
}

/// A directory that looks like a login of `agent`.
#[derive(Clone, Debug)]
pub struct Found {
    pub agent: &'static str,
    pub dir: PathBuf,
    pub traces: Vec<String>,
}

/// The directory where Windows keeps what was deleted, which holds no login.
const RECYCLE_BIN: &str = "$Recycle.Bin";

/// Whether a directory is where Windows keeps what was deleted; its name's
/// case varies by volume.
fn recycled(name: &std::ffi::OsStr) -> bool {
    cfg!(windows) && name.to_string_lossy().eq_ignore_ascii_case(RECYCLE_BIN)
}

/// A path as this machine compares it: on Windows without regard to case,
/// slash or the verbatim prefix.
pub fn key(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = if cfg!(windows) {
        let text = text.replace('/', "\\").to_lowercase();
        text.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(text)
    } else {
        text.into_owned()
    };
    let trimmed = text.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() || trimmed.ends_with(':') { text } else { trimmed.to_string() }
}

/// Walks `root`, skipping the directories whose `key` is in `skip`, until
/// done or `stop` is set; each directory that looks like a login goes to
/// `found` as it is met.
pub fn walk(root: &Path, skip: &HashSet<String>, stop: &AtomicBool, counters: &Counters, found: &mut dyn FnMut(Found)) {
    // The filesystem the walk stays on, where a filesystem can be mounted
    // inside another's directory.
    #[cfg(windows)]
    let device = 0;
    #[cfg(unix)]
    let device = {
        use std::os::unix::fs::MetadataExt as _;
        match std::fs::symlink_metadata(root) {
            Ok(metadata) => metadata.dev(),
            Err(_) => {
                counters.unreadable.fetch_add(1, Ordering::Relaxed);
                counters.done.store(true, Ordering::Release);
                return;
            }
        }
    };
    let mut pending: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let read = match std::fs::read_dir(&dir) {
            Ok(read) => read,
            // Gone since its parent was listed.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                counters.unreadable.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        let mut entries: Vec<Entry> = Vec::new();
        let mut below: Vec<OsString> = Vec::new();
        for entry in read {
            let Ok(entry) = entry else {
                counters.unreadable.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let name = entry.file_name();
            let Ok(kind) = entry.file_type() else {
                counters.unreadable.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let directory = kind.is_dir();
            entries.push((name.to_string_lossy().into_owned(), directory));
            if directory && walkable(&entry, device) && !recycled(&name) {
                below.push(name);
            }
        }
        counters.directories.fetch_add(1, Ordering::Relaxed);
        for (agent, _) in AGENTS {
            if let Some(traces) = traces::elsewhere(agent, &entries) {
                found(Found { agent, dir: dir.clone(), traces });
            }
        }
        for name in below.into_iter().rev() {
            let path = dir.join(name);
            if !skip.contains(&key(&path)) {
                pending.push(path);
            }
        }
    }
    counters.done.store(true, Ordering::Release);
}

/// Whether a directory entry is one to walk into: no link or reparse point
/// of any kind, and on Linux on the same filesystem as the walk's root.
#[cfg(windows)]
fn walkable(entry: &std::fs::DirEntry, _device: u64) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    /// `FILE_ATTRIBUTE_REPARSE_POINT`.
    const REPARSE_POINT: u32 = 0x400;
    // A directory entry's metadata comes from the listing itself.
    entry.metadata().is_ok_and(|metadata| metadata.file_attributes() & REPARSE_POINT == 0)
}

#[cfg(unix)]
fn walkable(entry: &std::fs::DirEntry, device: u64) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    entry.metadata().is_ok_and(|metadata| metadata.dev() == device)
}
