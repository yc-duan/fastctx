//! The content hash World file operations report: `copy` verifies what it wrote with it, and
//! the service compares installed binaries by it.

use crate::paths::display_path;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// Streams a file through SHA-256 and returns the lowercase hex digest.
pub(crate) fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("Cannot read {}: {error}", display_path(path)))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Cannot read {}: {error}", display_path(path)))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}
