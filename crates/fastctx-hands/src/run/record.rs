//! Reading and publishing a job's record.
//!
//! The node writes a job's first record; from then on the job's supervisor
//! is its only writer, holding the record's lock for as long as it lives.
//! Whoever later finds the lock free knows the supervisor is gone.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, publish};
use fastctx_types::run::JobRecord;

pub fn read(path: &Path) -> io::Result<JobRecord> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}

pub fn write(path: &Path, record: &JobRecord) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish(path, &bytes, Permissions::Private)
}

/// The lock a live supervisor holds on its job's record.
pub fn lock_path(record: &Path) -> PathBuf {
    record.with_extension("lock")
}
