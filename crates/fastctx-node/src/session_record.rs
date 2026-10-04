//! Session events in their durable acceptance order. The caller holds the
//! resident storage lock, including while allocating a sequence number.

use std::io;
use std::path::Path;

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::session::SessionEvent;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Recorded {
    #[serde(default)]
    sequence: Option<u64>,
    #[serde(flatten)]
    event: SessionEvent,
}

pub fn append(directory: &Path, key: &str, event: &SessionEvent) -> io::Result<bool> {
    let records = directory.join("session-events");
    let path = records.join(format!("{key}.json"));
    if path.try_exists()? {
        return Ok(false);
    }
    ensure_private_dir(&records)?;
    let counter = directory.join("session-events-sequence.json");
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

pub fn history(directory: &Path) -> io::Result<Vec<SessionEvent>> {
    let mut records = read(&directory.join("session-events"))?;
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

/// Reconcile the allocator after a resident's event files were carried to
/// another town, including a return to a directory with an older counter.
pub fn carried(from: &Path, to: &Path) -> io::Result<()> {
    let mut sequence =
        read(&to.join("session-events"))?.iter().filter_map(|(_, record)| record.sequence).max().unwrap_or(0);
    for directory in [from, to] {
        match std::fs::read(directory.join("session-events-sequence.json")) {
            Ok(bytes) => sequence = sequence.max(serde_json::from_slice(&bytes).map_err(io::Error::other)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    ensure_private_dir(to)?;
    publish(
        &to.join("session-events-sequence.json"),
        &serde_json::to_vec(&sequence).map_err(io::Error::other)?,
        Permissions::Private,
    )
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
