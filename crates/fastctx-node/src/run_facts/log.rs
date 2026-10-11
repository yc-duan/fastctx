//! Serialized callers append complete JSON rows under their record lock.

use std::io::{self, Seek, Write};
use std::path::Path;

pub(super) fn append(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new().create(true).read(true).write(true).truncate(false).open(path)?;
    let before = file.seek(io::SeekFrom::End(0))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        file.set_len(before)
            .and_then(|()| file.sync_all())
            .map_err(|rollback| io::Error::other(format!("{error}; restoring the previous log length: {rollback}")))?;
        return Err(error);
    }
    Ok(())
}
