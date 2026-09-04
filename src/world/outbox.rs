//! The member's persistent outbox: reliable envelopes written before they are sent and
//! deleted only when the hub acknowledges their sequence number.

use super::envelope::Envelope;
use super::{WorldPaths, write_atomic};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Reliable messages a member queues before refusing new ones.
pub(crate) const OUTBOX_LIMIT: usize = 10_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct OutboxEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<u64>,
    pub(crate) env: Envelope,
    pub(crate) queued_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Outbox {
    directory: PathBuf,
}

impl Outbox {
    pub(crate) fn new(paths: &WorldPaths) -> Self {
        Self {
            directory: paths.outbox_dir.clone(),
        }
    }

    fn path(&self, seq: u64) -> PathBuf {
        self.directory.join(format!("{seq:020}.json"))
    }

    /// Stores one message under its sequence number.
    pub(crate) fn push(&self, seq: u64, entry: &OutboxEntry) -> Result<(), String> {
        if self.depth()? >= OUTBOX_LIMIT {
            return Err(format!(
                "outbox_full: {OUTBOX_LIMIT} messages are already waiting for the hub."
            ));
        }
        let json = serde_json::to_vec(entry)
            .map_err(|error| format!("Cannot encode an outbox entry: {error}"))?;
        write_atomic(&self.path(seq), &json)
    }

    /// Removes every message at or below `seq`.
    pub(crate) fn ack(&self, seq: u64) -> Result<(), String> {
        for (queued, _) in self.entries()? {
            if queued <= seq {
                let path = self.path(queued);
                if let Err(error) = std::fs::remove_file(&path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        return Err(format!(
                            "Cannot remove {}: {error}",
                            crate::paths::display_path(&path)
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Every queued message in sequence order.
    pub(crate) fn entries(&self) -> Result<Vec<(u64, OutboxEntry)>, String> {
        let mut entries = Vec::new();
        let listing = match std::fs::read_dir(&self.directory) {
            Ok(listing) => listing,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
            Err(error) => {
                return Err(format!(
                    "Cannot list {}: {error}",
                    crate::paths::display_path(&self.directory)
                ));
            }
        };
        for item in listing.flatten() {
            let path = item.path();
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Ok(seq) = stem.parse::<u64>() else {
                continue;
            };
            let bytes = std::fs::read(&path).map_err(|error| {
                format!("Cannot read {}: {error}", crate::paths::display_path(&path))
            })?;
            let entry: OutboxEntry = serde_json::from_slice(&bytes).map_err(|error| {
                format!(
                    "Cannot parse {}: {error}",
                    crate::paths::display_path(&path)
                )
            })?;
            entries.push((seq, entry));
        }
        entries.sort_by_key(|(seq, _)| *seq);
        Ok(entries)
    }

    pub(crate) fn depth(&self) -> Result<usize, String> {
        Ok(self.entries()?.len())
    }
}
