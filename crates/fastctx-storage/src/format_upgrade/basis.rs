//! Inspect the owned database and its WAL before startup can create either file.
use std::{fs, io, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupBasis {
    NewDatabase,
    ExistingDatabase { database_bytes: u64, wal_bytes: u64 },
}
impl BackupBasis {
    /// The startup owner keeps exclusive ownership from this observation to prepare.
    /// Even an empty existing file is conservatively backed up; WAL-only data counts.
    pub fn observe(path: &Path) -> io::Result<Self> {
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let database = length(path)?;
        let log = length(Path::new(&wal))?;
        if database.is_none() && log.is_none() {
            Ok(Self::NewDatabase)
        } else {
            Ok(Self::ExistingDatabase { database_bytes: database.unwrap_or(0), wal_bytes: log.unwrap_or(0) })
        }
    }
    pub const fn needs_backup(self) -> bool {
        matches!(self, Self::ExistingDatabase { .. })
    }
}
fn length(path: &Path) -> io::Result<Option<u64>> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
        Ok(_) => Err(io::Error::new(io::ErrorKind::InvalidInput, "upgrade source is not a regular database file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
