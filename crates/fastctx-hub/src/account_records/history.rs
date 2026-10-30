//! Preserve the previous plan before replacing its current record. A crash
//! between these publications leaves the current version and its exact archive.

use fastctx_types::account_records::Record;
use std::{io, path::Path};

pub(super) fn preserve(current: &Path, directory: &Path) -> io::Result<()> {
    let bytes = match std::fs::read(current) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let record: Record = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    fastctx_platform::ensure_private_dir(directory)?;
    let path = directory.join(format!("{}.json", record.metadata.version));
    match std::fs::read(&path) {
        Ok(old) if old == bytes => Ok(()),
        Ok(_) => Err(io::Error::other("the plan's archived version differs from its current version")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
        }
        Err(error) => Err(error),
    }
}
