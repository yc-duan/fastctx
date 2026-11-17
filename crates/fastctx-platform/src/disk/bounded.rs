//! Size-limited regular-file reads retain disk role and account cost tracking.

use std::{
    io::{self, Read},
    path::Path,
};

/// Reads a regular file within `limit` bytes, rejecting a larger file.
/// The extra byte detects growth after metadata was read.
pub fn read_bounded(path: impl AsRef<Path>, limit: usize) -> io::Result<Vec<u8>> {
    super::in_place(|| {
        let file = super::open_regular(path)?;
        let too_large =
            || io::Error::new(io::ErrorKind::InvalidData, format!("file exceeds the {limit} byte read limit"));
        if file.metadata()?.len() > limit as u64 {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        file.take((limit as u64).saturating_add(1)).read_to_end(&mut bytes)?;
        crate::cost::read(bytes.len());
        if bytes.len() > limit {
            return Err(too_large());
        }
        Ok(bytes)
    })
}
