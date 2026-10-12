//! Session events in their durable acceptance order. The caller holds the
//! resident storage lock, including while allocating a sequence number.

use std::io;
use std::path::Path;

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

pub fn append(directory: &ConversationDir, key: &str, event: &SessionEvent) -> io::Result<bool> {
    let records = directory.session_events();
    let path = records.join(format!("{key}.json"));
    if path.try_exists()? {
        return Ok(false);
    }
    ensure_private_dir(&records)?;
    let counter = directory.session_sequence();
    let previous: u64 = match std::fs::read(&counter) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            read(&records)?.iter().filter_map(|(_, record)| record.sequence).max().unwrap_or(0)
        }
        Err(error) => return Err(error),
    };
    let sequence = previous.checked_add(1).ok_or_else(|| io::Error::other("session event sequence exhausted"))?;
    // Reserve before publishing. A crash can leave a gap, never duplicate
    // the position of an event that already reached disk.
    publish(&counter, &serde_json::to_vec(&sequence).map_err(io::Error::other)?, Permissions::Private)?;
    let record = Recorded { sequence: Some(sequence), event: event.clone() };
    publish_new(&path, &serde_json::to_vec(&record).map_err(io::Error::other)?, Permissions::Private)?;
    Ok(true)
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
