//! Session events in their durable acceptance order. The caller holds the
//! resident storage lock, including while allocating a sequence number.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::layout::ConversationDir;
use fastctx_types::session::SessionEvent;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Recorded {
    #[serde(default)]
    sequence: Option<u64>,
    #[serde(flatten)]
    event: SessionEvent,
}

/// Positions are reserved this many at a time: the counter on disk names
/// the last one reserved, not the last one used.
const RESERVED: u64 = 64;

/// For each conversation's counter, the next position to give and the last
/// one this node reserved there, as it last wrote the counter.
static RESERVATIONS: Mutex<Option<HashMap<PathBuf, (u64, u64)>>> = Mutex::new(None);

pub fn append(directory: &ConversationDir, key: &str, event: &SessionEvent) -> io::Result<bool> {
    let records = directory.session_events();
    let path = records.join(format!("{key}.json"));
    if path.try_exists()? {
        return Ok(false);
    }
    ensure_private_dir(&records)?;
    let counter = directory.session_sequence();
    let written: Option<u64> = match std::fs::read(&counter) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(io::Error::other)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut reservations = RESERVATIONS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let reservations = reservations.get_or_insert_with(HashMap::new);
    // A counter that is not as this node last left it was reset, moved or
    // advanced by someone else: positions start again after what it says,
    // or after every event on record where it is missing.
    let (next, reserved) = match reservations.get(&counter) {
        Some(&(next, reserved)) if written == Some(reserved) => (next, reserved),
        _ => {
            let previous = match written {
                Some(previous) => previous,
                None => read(&records)?.iter().filter_map(|(_, record)| record.sequence).max().unwrap_or(0),
            };
            (previous.checked_add(1).ok_or_else(exhausted)?, previous)
        }
    };
    // Reserve before publishing. A crash can leave a gap, never duplicate
    // the position of an event that already reached disk.
    let reserved = if next > reserved {
        let until = next.checked_add(RESERVED - 1).ok_or_else(exhausted)?;
        publish(&counter, &serde_json::to_vec(&until).map_err(io::Error::other)?, Permissions::Private)?;
        until
    } else {
        reserved
    };
    reservations.insert(counter, (next + 1, reserved));
    let record = Recorded { sequence: Some(next), event: event.clone() };
    publish_new(&path, &serde_json::to_vec(&record).map_err(io::Error::other)?, Permissions::Private)?;
    Ok(true)
}

/// Forgets what this node reserved at a counter someone else just wrote, so
/// the next position follows what it says.
pub(crate) fn reset(counter: &Path) {
    if let Some(reservations) = RESERVATIONS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_mut() {
        reservations.remove(counter);
    }
}

fn exhausted() -> io::Error {
    io::Error::other("session event sequence exhausted")
}

pub fn history(directory: &ConversationDir) -> io::Result<Vec<SessionEvent>> {
    let mut records = read(&directory.session_events())?;
    // Old files cannot tell the order of equal timestamps. Their filename
    // supplies a deterministic tie break; new files carry the actual order.
    records.sort_by(|(left_name, left), (right_name, right)| {
        left.sequence
            .cmp(&right.sequence)
            .then_with(|| left.event.at.cmp(&right.event.at))
            .then_with(|| left_name.cmp(right_name))
    });
    Ok(records.into_iter().map(|(_, record)| record.event).collect())
}

fn read(directory: &Path) -> io::Result<Vec<(String, Recorded)>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut records = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "json")
            && !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            let record = serde_json::from_slice(&std::fs::read(&path)?).map_err(io::Error::other)?;
            records.push((path.file_name().expect("directory entry").to_string_lossy().into_owned(), record));
        }
    }
    Ok(records)
}

/// What `history` reads, told by the files it reads: each event is a file
/// written once.
pub fn stamp(directory: &Path, digest: &mut impl FnMut(&[u8])) -> io::Result<()> {
    stamp_files(&directory.join("session-events"), digest)
}

/// The files of `directory` by name, size and time of change, in name order;
/// a directory that is not there is told as empty.
pub fn stamp_files(directory: &Path, digest: &mut impl FnMut(&[u8])) -> io::Result<()> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let meta = entry.metadata()?;
        files.push(format!("{}|{}|{:?}", entry.file_name().to_string_lossy(), meta.len(), meta.modified().ok()));
    }
    files.sort();
    for file in files {
        digest(file.as_bytes());
    }
    Ok(())
}
