//! Letters on this machine: those waiting to reach the hub, the receipts
//! of those taken, and the ones that could not be delivered or opened.

use std::io;
use std::path::PathBuf;

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::MachineName;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::state::{NetDir, read};
use crate::envelope::Envelope;
use crate::message::LetterId;
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

    /// Forgets the letters through `seq`: the hub holds them.
    pub fn held_through(&self, seq: u64) -> io::Result<()> {
        for number in self.numbers()? {
            if number > seq {
                break;
            }
            match std::fs::remove_file(self.dir.join(format!("{number:020}"))) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
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

pub fn receipt(net: &NetDir, from: &MachineName, id: &LetterId) -> io::Result<Option<Receipt>> {
    match read(&net.received(from)?.join(id.as_str())) {
        Ok(receipt) => Ok(Some(receipt)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn keep_receipt(net: &NetDir, from: &MachineName, id: &LetterId, receipt: &Receipt) -> io::Result<()> {
    let directory = net.received(from)?;
    ensure_private_dir(&directory)?;
    let bytes = serde_json::to_vec(receipt).map_err(io::Error::other)?;
    publish(&directory.join(id.as_str()), &bytes, Permissions::Private)
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

pub fn record_failure(net: &NetDir, direction: &str, reason: &str, message: &Envelope) -> io::Result<PathBuf> {
    let at = Timestamp::now();
    let failure =
        Failure { at, direction: direction.to_string(), reason: reason.to_string(), message: message.clone() };
    let stamp = at.strftime("%Y%m%dT%H%M%S%.fZ").to_string().replace('.', "");
    let suffix = u32::from_le_bytes(crate::sign::random::<4>()?);
    let path = net.failures().join(format!("{stamp}-{direction}-{suffix:08x}.json"));
    let mut bytes = serde_json::to_vec_pretty(&failure).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish_new(&path, &bytes, Permissions::Private)?;
    Ok(path)
}
