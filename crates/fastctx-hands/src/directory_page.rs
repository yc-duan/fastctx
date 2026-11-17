//! Disposable directory iterators. Continuations resume enumeration without
//! rescanning earlier entries; names are sorted only within each returned page.

use std::collections::{HashMap, VecDeque};
use std::fs::{DirEntry, ReadDir};
use std::io;
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const ENTRIES: usize = 1024;
const CURSORS: usize = 64;
const IDLE: Duration = Duration::from_secs(120);
const SLICE: Duration = Duration::from_millis(100);

struct Cursor {
    identity: fastctx_platform::file_read::FileIdentity,
    path: PathBuf,
    entries: Option<Peekable<ReadDir>>,
    pending: VecDeque<DirEntry>,
    used: Instant,
}

static KEPT: LazyLock<Mutex<HashMap<String, Cursor>>> = LazyLock::new(Default::default);

pub(crate) struct Page {
    pub identity: fastctx_platform::file_read::FileIdentity,
    pub entries: Vec<DirEntry>,
    pub next: Option<String>,
}

pub(crate) fn read(path: &Path, after: Option<&str>) -> io::Result<Page> {
    let identity = identity(path)?;
    let now = Instant::now();
    let retired = {
        let mut kept = KEPT.lock().expect("directory cursors poisoned");
        let expired: Vec<_> = kept
            .iter()
            .filter(|(_, cursor)| now.duration_since(cursor.used) >= IDLE)
            .map(|(id, _)| id.clone())
            .collect();
        expired.into_iter().filter_map(|id| kept.remove(&id)).collect::<Vec<_>>()
    };
    drop(retired);
    let mut kept = KEPT.lock().expect("directory cursors poisoned");
    let mut cursor = if let Some(after) = after {
        let cursor =
            kept.remove(after).ok_or_else(|| io::Error::other("the directory cursor expired; reopen the folder"))?;
        if cursor.path != path {
            kept.insert(after.to_owned(), cursor);
            return Err(io::Error::other("the directory cursor belongs to another folder"));
        }
        if cursor.identity != identity {
            drop(kept);
            return Err(io::Error::other("the folder changed identity; reopen it"));
        }
        cursor
    } else {
        if kept.len() >= CURSORS {
            return Err(io::Error::other("too many directory pages are open; retry after they expire"));
        }
        drop(kept);
        let entries = std::fs::read_dir(path)?.peekable();
        kept = KEPT.lock().expect("directory cursors poisoned");
        Cursor {
            identity: identity.clone(),
            path: path.to_owned(),
            entries: Some(entries),
            pending: VecDeque::new(),
            used: now,
        }
    };
    drop(kept);
    let mut entries = Vec::new();
    while entries.len() < ENTRIES && (entries.is_empty() || now.elapsed() < SLICE) {
        let Some(entry) =
            cursor.pending.pop_front().map(Ok).or_else(|| cursor.entries.as_mut().and_then(Iterator::next))
        else {
            break;
        };
        entries.push(entry?);
    }
    let next = if !cursor.pending.is_empty() || cursor.entries.as_mut().is_some_and(|entries| entries.peek().is_some())
    {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(io::Error::other)?;
        let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        cursor.used = Instant::now();
        let mut kept = KEPT.lock().expect("directory cursors poisoned");
        if kept.len() >= CURSORS {
            return Err(io::Error::other("too many directory pages are open; reopen the folder"));
        }
        kept.insert(id.clone(), cursor);
        Some(id)
    } else {
        None
    };
    unchanged(path, &identity)?;
    Ok(Page { identity, entries, next })
}

/// Retains names counted but not displayed by an outline of the current page.
pub(crate) fn prepend(
    path: &Path,
    entries: Vec<DirEntry>,
    next: Option<String>,
    expected: &fastctx_platform::file_read::FileIdentity,
) -> io::Result<String> {
    unchanged(path, expected)?;
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut cursor = {
        let mut kept = KEPT.lock().expect("directory cursors poisoned");
        if let Some(next) = next {
            kept.remove(&next).ok_or_else(|| io::Error::other("the directory cursor expired; reopen the folder"))?
        } else {
            Cursor {
                identity: expected.clone(),
                path: path.to_owned(),
                entries: None,
                pending: VecDeque::new(),
                used: Instant::now(),
            }
        }
    };
    if cursor.path != path {
        return Err(io::Error::other("the directory cursor belongs to another folder"));
    }
    if entries.len() + cursor.pending.len() > ENTRIES {
        return Err(io::Error::other("too many retained directory entries"));
    }
    let mut pending: VecDeque<_> = entries.into();
    pending.append(&mut cursor.pending);
    cursor.pending = pending;
    let mut kept = KEPT.lock().expect("directory cursors poisoned");
    if kept.len() >= CURSORS {
        return Err(io::Error::other("too many directory pages are open; reopen the folder"));
    }
    cursor.used = Instant::now();
    kept.insert(id.clone(), cursor);
    Ok(id)
}

fn identity(path: &Path) -> io::Result<fastctx_platform::file_read::FileIdentity> {
    let file = fastctx_platform::file_read::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() {
        return Err(io::Error::other("the path is no longer a folder"));
    }
    Ok(fastctx_platform::file_read::snapshot(&file, &metadata)?.identity())
}

pub(crate) fn unchanged(path: &Path, expected: &fastctx_platform::file_read::FileIdentity) -> io::Result<()> {
    if identity(path)? != *expected {
        return Err(io::Error::other("the folder changed identity; reopen it"));
    }
    Ok(())
}
