//! Durable outbound envelopes with an in-memory index of the files still awaiting an ACK.
use super::{WorldPaths, envelope::Envelope, write_atomic};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct OutboxEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<u64>,
    pub(crate) env: Envelope,
    pub(crate) queued_at: String,
}
#[derive(Debug)]
pub(crate) struct Outbox {
    directory: PathBuf,
    sequences: Mutex<BTreeSet<u64>>,
}
impl Outbox {
    pub(crate) fn new(paths: &WorldPaths) -> Result<Self, String> {
        let directory = paths.outbox_dir.clone();
        let mut sequences = BTreeSet::new();
        match std::fs::read_dir(&directory) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry
                        .map_err(|e| format!("Cannot list outbox: {e}"))?
                        .path();
                    if let Some(seq) = path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .and_then(|name| name.parse().ok())
                    {
                        sequences.insert(seq);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot list {}: {error}", directory.display())),
        }
        Ok(Self {
            directory,
            sequences: Mutex::new(sequences),
        })
    }
    fn path(&self, seq: u64) -> PathBuf {
        self.directory.join(format!("{seq:020}.json"))
    }
    pub(crate) fn push(&self, seq: u64, entry: &OutboxEntry) -> Result<(), String> {
        let mut sequences = self.sequences.lock();
        if sequences.contains(&seq) {
            return Err(format!("Outbox sequence {seq} is already committed"));
        }
        let bytes = serde_json::to_vec(entry).map_err(|e| e.to_string())?;
        write_atomic(&self.path(seq), &bytes)?;
        sequences.insert(seq);
        Ok(())
    }
    pub(crate) fn ack(&self, seq: u64) -> Result<(), String> {
        let mut sequences = self.sequences.lock();
        let acknowledged = sequences.range(..=seq).copied().collect::<Vec<_>>();
        for queued in acknowledged {
            match std::fs::remove_file(self.path(queued)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Cannot acknowledge outbox sequence {queued}: {e}")),
            }
            sequences.remove(&queued);
        }
        Ok(())
    }
    pub(crate) fn entries(&self) -> Result<Vec<(u64, OutboxEntry)>, String> {
        self.sequences
            .lock()
            .iter()
            .map(|seq| {
                let path = self.path(*seq);
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
                let entry = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("Cannot parse {}: {e}", path.display()))?;
                Ok((*seq, entry))
            })
            .collect()
    }
    pub(crate) fn depth(&self) -> usize {
        self.sequences.lock().len()
    }
}
