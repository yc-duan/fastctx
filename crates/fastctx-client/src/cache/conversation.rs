//! One conversation's history as this device keeps it: the segments its
//! machine cut it into, each in a file of its own, and an index that says
//! which make up the history now and which version that is.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write as _};
use std::path::PathBuf;

use fastctx_platform::{FileLock, Permissions, publish};
use fastctx_types::Address;
use fastctx_types::session::SessionEvent;
use fastctx_types::session::held::SegmentId;
use serde::{Deserialize, Serialize};

/// Which segments make up a conversation's history, as last brought up to
/// date.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Index {
    pub address: Address,
    /// The version its machine named; none where that machine names none.
    pub version: Option<String>,
    pub layout: Vec<SegmentId>,
    /// How many events each segment of the layout holds.
    pub events: Vec<usize>,
    #[serde(default)]
    pub boundaries: Vec<bool>,
    /// The pictures the history shows, by their keys (`super::pictures`).
    pub pictures: Vec<String>,
    #[serde(default)]
    pub blobs: Vec<String>,
    #[serde(default)]
    pub live_sequence: u64,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub rewrite: Option<String>,
    pub updated: jiff::Timestamp,
    /// Its machine said the history was rewritten since.
    #[serde(default)]
    pub stale: bool,
}

/// Where one conversation is kept.
#[derive(Clone)]
pub(crate) struct Entry {
    directory: PathBuf,
}

/// A history put together from what came and what was kept.
pub(crate) struct Whole {
    pub events: Vec<SessionEvent>,
    /// How many events each segment of the layout holds.
    pub counts: Vec<usize>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Live {
    #[serde(default)]
    pub sequence: u64,
    #[serde(flatten)]
    pub event: SessionEvent,
}

impl Entry {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// The index, if the conversation is kept. Its live sequence is the
    /// later of the index's own and the one kept beside it, which moves
    /// with every event kept while the index is written only as what it
    /// lists changes (`set_live_sequence`).
    pub fn index(&self) -> io::Result<Option<Index>> {
        let mut index: Index = match std::fs::read(self.directory.join("index.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", self.directory.display()))
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        match std::fs::read(self.directory.join("live-sequence.json")) {
            Ok(bytes) => {
                let kept: u64 = serde_json::from_slice(&bytes).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", self.directory.display()))
                })?;
                index.live_sequence = index.live_sequence.max(kept);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(Some(index))
    }

    /// The sequence of the latest live event, before the event is written.
    pub fn set_live_sequence(&self, sequence: u64) -> io::Result<()> {
        publish(&self.directory.join("live-sequence.json"), &serde_json::to_vec(&sequence)?, Permissions::Private)
    }

    /// The events of a kept segment. The file's checksum tells a damaged
    /// copy, which is an error.
    pub fn segment(&self, id: &SegmentId) -> io::Result<Vec<SessionEvent>> {
        let path = self.path(id);
        let bytes = std::fs::read(&path)?;
        fastctx_net::history::unpack(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
    }

    pub fn live(&self) -> io::Result<Vec<SessionEvent>> {
        let mut seen = HashSet::new();
        Ok(self
            .live_records()?
            .into_iter()
            .filter_map(|record| {
                seen.insert(serde_json::to_vec(&record.event).expect("session events serialize"))
                    .then_some(record.event)
            })
            .collect())
    }

    pub fn live_records(&self) -> io::Result<Vec<Live>> {
        let bytes = match std::fs::read(self.directory.join("live.jsonl")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)))
            .collect()
    }

    pub fn record(&self, event: &SessionEvent, sequence: u64) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&Live { sequence, event: event.clone() })?;
        bytes.push(b'\n');
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(self.directory.join("live.jsonl"))?;
        file.write_all(&bytes)?;
        file.sync_data()
    }

    pub fn keep_live(&self, live: &[Live]) -> io::Result<()> {
        if live.is_empty() {
            return self.clear_live();
        }
        let mut bytes = Vec::new();
        for record in live {
            serde_json::to_writer(&mut bytes, record)?;
            bytes.push(b'\n');
        }
        publish(&self.directory.join("live.jsonl"), &bytes, Permissions::Private)
    }

    pub fn clear_live(&self) -> io::Result<()> {
        match std::fs::remove_file(self.directory.join("live.jsonl")) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Writers of one conversation take turns, from the first segment they
    /// keep to the index that names it.
    pub fn turn(&self) -> io::Result<FileLock> {
        fastctx_platform::ensure_private_dir(&self.directory)?;
        FileLock::acquire(&self.directory.join(".lock"))
    }

    /// Keeps the segments of `fresh` that are not kept yet, then puts the
    /// history `layout` names together from them and from those kept before.
    /// Call it in turn (`Entry::turn`).
    pub fn put_together(
        &self,
        layout: &[SegmentId],
        mut fresh: HashMap<SegmentId, Vec<SessionEvent>>,
    ) -> io::Result<Whole> {
        for (id, events) in &fresh {
            let path = self.path(id);
            if !path.try_exists()? {
                publish(&path, &fastctx_net::history::pack(events), Permissions::Private)?;
            }
        }
        let mut whole = Whole { events: Vec::new(), counts: Vec::with_capacity(layout.len()) };
        for id in layout {
            let events = match fresh.remove(id) {
                Some(events) => events,
                None => self.segment(id)?,
            };
            whole.counts.push(events.len());
            whole.events.extend(events);
        }
        Ok(whole)
    }

    /// Keeps `index`, and lets go of the segments it no longer names. Call
    /// it in turn (`Entry::turn`).
    pub fn keep_index(&self, index: &Index) -> io::Result<()> {
        self.write_index(index)?;
        let named: HashSet<String> = index.layout.iter().map(|id| format!("{id}.zst")).collect();
        for entry in std::fs::read_dir(&self.directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".zst") && !named.contains(&name) {
                match std::fs::remove_file(entry.path()) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub fn write_index(&self, index: &Index) -> io::Result<()> {
        publish(&self.directory.join("index.json"), &serde_json::to_vec(index)?, Permissions::Private)
    }

    pub fn remove(&self) -> io::Result<()> {
        match std::fs::remove_dir_all(&self.directory) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn path(&self, id: &SegmentId) -> PathBuf {
        self.directory.join(format!("{id}.zst"))
    }
}
