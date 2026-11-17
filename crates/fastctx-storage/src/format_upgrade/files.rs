//! Only names recorded by an attempt can be cleaned; complete backups are retained.
use super::{Record, names};
use crate::{Error, Result};
use fastctx_platform::{atomic, rooted_path::Directory};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn now() -> Result<i64> {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| Error::new(e.to_string()))?.as_millis();
    i64::try_from(millis).map_err(|_| Error::new("upgrade timestamp overflow"))
}
pub(super) fn name(record: &Record, attempt: u8) -> Result<String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    Ok(format!(
        "{}-format-{}-{}-t{}-{}-{}.sqlite3",
        record.service.name(),
        record.previous_format.map_or("unknown".into(), |n| n.to_string()),
        now()?,
        attempt,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
#[derive(Clone)]
pub(super) struct Files {
    pub directory: PathBuf,
}
impl Files {
    pub fn new(database: &Path) -> Result<Self> {
        let root = database.parent().ok_or_else(|| Error::new("database has no root"))?;
        Ok(Self { directory: root.join("upgrade-backups") })
    }
    pub fn prepare(&self) -> Result<()> {
        // No held directory capability survives an I/O batch: on Windows it also
        // keeps ancestors from being renamed while held.
        Directory::open_root(&self.directory, true)?;
        Ok(())
    }
    pub fn final_path(&self, record: &Record) -> Result<PathBuf> {
        let name = record.file_name.as_deref().ok_or_else(|| Error::new("attempt has no file name"))?;
        if !names::valid(name) {
            return Err(Error::new("invalid backup name"));
        }
        Ok(self.directory.join(name))
    }
    pub fn partial(&self, record: &Record) -> Result<PathBuf> {
        let mut path = self.final_path(record)?.into_os_string();
        path.push(".partial");
        Ok(path.into())
    }
    pub fn create(&self, record: &Record) -> Result<PathBuf> {
        let path = self.partial(record)?;
        if self.final_path(record)?.try_exists()? {
            return Err(Error::new("backup name already exists"));
        }
        atomic::publish_new(&path, &[], atomic::Permissions::Private)?;
        Ok(path)
    }
    pub fn copied_path(&self, record: &Record) -> Result<PathBuf> {
        let final_path = self.final_path(record)?;
        // A crash after publication but before Complete keeps Copied evidence.
        let partial = self.partial(record)?;
        let path = if final_path.try_exists()? {
            if partial.try_exists()? {
                let final_file = fs::File::open(&final_path)?;
                let partial_file = fs::File::open(&partial)?;
                if fastctx_platform::rooted_path::identity(&final_file)?
                    != fastctx_platform::rooted_path::identity(&partial_file)?
                {
                    return Err(Error::new("backup destination collided with another file"));
                }
            }
            final_path
        } else {
            partial
        };
        if !fs::symlink_metadata(&path)?.is_file() {
            return Err(Error::new("backup is not a regular file"));
        }
        Ok(path)
    }
    pub fn publish(&self, record: &Record) -> Result<()> {
        let target = self.final_path(record)?;
        if target.try_exists()? {
            atomic::sync_existing(&target)?;
        } else {
            atomic::publish_existing_new(&self.partial(record)?, &target)?;
        }
        Ok(())
    }
    pub fn clean_partial(&self, record: &Record) -> Result<()> {
        if record.file_name.is_none() {
            return Ok(());
        }
        // SQLite may leave private sidecars if killed during Backup or close.
        let path = self.partial(record)?;
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let mut file = path.clone().into_os_string();
            file.push(suffix);
            match fs::remove_file(PathBuf::from(file)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        if self.directory.try_exists()? {
            Directory::open_root(&self.directory, false)?.sync_chain()?;
        }
        Ok(())
    }
}
