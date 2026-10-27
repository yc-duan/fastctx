//! Letters on this machine: those waiting to reach the hub, the receipts
//! of those taken, and the ones that could not be delivered or opened.

mod failures;
mod receipts;
mod taking;
pub use failures::record_failure;
pub(super) use receipts::{acknowledge, start};
pub(super) use taking::take;

use std::io;
use std::path::PathBuf;

use fastctx_platform::{Permissions, publish, publish_new};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::state::{NetDir, read};
use crate::envelope::Envelope;

use crate::sign::Digest;

/// Letters not yet held by the hub, one file each, named by number.
pub struct Outbox {
    dir: PathBuf,
}

impl Outbox {
    pub fn new(net: &NetDir) -> Self {
        Self { dir: net.outbox() }
    }

    pub fn put(&self, seq: u64, message: &Envelope) -> io::Result<()> {
        let bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
        publish_new(&self.dir.join(format!("{seq:020}")), &bytes, Permissions::Private)
    }

    pub fn get(&self, seq: u64) -> io::Result<Option<Envelope>> {
        match read(&self.dir.join(format!("{seq:020}"))) {
            Ok(message) => Ok(Some(message)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The letter `seq` and how many bytes it takes; none when it is not
    /// there.
    pub fn get_sized(&self, seq: u64) -> io::Result<Option<(Envelope, usize)>> {
        let bytes = match std::fs::read(self.dir.join(format!("{seq:020}"))) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let message =
            serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Some((message, bytes.len())))
    }

    /// Forgets the letters after `from` through `through`, which the hub
    /// holds now, by number: letters go one after another, so no listing is
    /// needed.
    pub fn held_between(&self, from: u64, through: u64) -> io::Result<()> {
        for number in from.saturating_add(1)..=through {
            self.remove(number)?;
        }
        Ok(())
    }

    /// Forgets the letter `seq`, which the hub holds.
    pub fn remove(&self, seq: u64) -> io::Result<()> {
        match std::fs::remove_file(self.dir.join(format!("{seq:020}"))) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// The numbers of the letters waiting, in order.
    pub fn numbers(&self) -> io::Result<Vec<u64>> {
        let mut numbers = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            // Temporary files of an interrupted write start with a dot.
            if let Some(seq) = entry.file_name().to_str().and_then(|name| name.parse::<u64>().ok()) {
                numbers.push(seq);
            }
        }
        numbers.sort_unstable();
        Ok(numbers)
    }
}

/// What this machine knows of a letter it took: the digest of its contents,
/// so the same letter sent again is taken once and another letter under the
/// same id is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub digest: Digest,
    pub at: Timestamp,
}


/// A letter that could not be delivered or opened, kept with the reason.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Failure {
    pub at: Timestamp,
    /// `in` for a letter this machine received, `out` for one it sent.
    pub direction: String,
    pub reason: String,
    pub message: Envelope,
}
