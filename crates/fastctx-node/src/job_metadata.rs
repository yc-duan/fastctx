//! Node-owned job metadata is scoped to the full job incarnation. Supervisor
//! records and byte streams remain files; a reused short job ID inherits nothing.

use crate::records::{self, Rows, Source};
use fastctx_storage::{Priority, Result};
use fastctx_types::{layout::ConversationDir, run::JobRecord};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(crate) struct Job {
    directory: ConversationDir,
    prefix: String,
    record: PathBuf,
    sequence_key: String,
}

impl Job {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        Self::of(path, &fastctx_hands::run::record::read(path)?)
    }
    pub(crate) fn of(path: &Path, record: &JobRecord) -> io::Result<Self> {
        let (directory, family, key, _) = crate::record_database::item(path)?;
        let sequence_key = format!("{:x}", Sha256::digest(serde_json::to_vec(&(&family, &key))?));
        let identity = serde_json::to_vec(&(family, key, record.started_at, &record.owner))?;
        let prefix = format!("jobs/{:x}/", Sha256::digest(identity));
        Ok(Self { directory, prefix, record: path.to_path_buf(), sequence_key })
    }
    pub(crate) fn next_sequence(&self, minimum: u64) -> io::Result<u64> {
        let key = self.sequence_key.clone();
        records::write_at(&self.directory, Priority::Normal, 64, move |rows| {
            let prior = rows.get::<u64>("job-sequences", &key)?.unwrap_or(0).max(minimum);
            let next =
                prior.checked_add(1).ok_or_else(|| fastctx_storage::Error::new("job facts sequence exhausted"))?;
            rows.put("job-sequences", &key, &next)?;
            Ok(next)
        })
    }
    pub(crate) fn family(&self, kind: &str) -> String {
        format!("{}{kind}", self.prefix)
    }
    pub(crate) fn prepare(
        &self,
        kind: &str,
        source: impl FnOnce(&mut dyn FnMut(Source) -> io::Result<()>) -> io::Result<()>,
    ) -> io::Result<()> {
        records::ensure_each(&self.directory, &self.family(kind), source)
    }
    pub(crate) fn scalar(&self, kind: &str) -> io::Result<()> {
        self.prepare(kind, |push| {
            let path = self.record.with_extension(kind);
            match std::fs::read(&path) {
                Ok(bytes) => push(Source::new("value".into(), path, bytes)?),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        })
    }
    pub(crate) fn read<T>(&self, kind: &str, action: impl FnOnce(&Rows<'_>, &str) -> Result<T>) -> io::Result<T> {
        records::read(&self.directory, |rows| action(rows, &self.family(kind)))
    }
    pub(crate) fn change<T: Send + 'static>(
        &self,
        kind: &str,
        bytes: usize,
        action: impl FnOnce(&Rows<'_>, &str) -> Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let family = self.family(kind);
        records::write_at(&self.directory, Priority::Normal, bytes, move |rows| action(rows, &family))
    }
    pub(crate) fn get<T: DeserializeOwned>(&self, kind: &str) -> io::Result<Option<T>> {
        self.scalar(kind)?;
        self.read(kind, |rows, family| rows.get(family, "value"))
    }
    pub(crate) fn raw(&self, kind: &str) -> io::Result<Option<Vec<u8>>> {
        self.scalar(kind)?;
        self.read(kind, |rows, family| rows.get_raw(family, "value"))
    }
    pub(crate) fn put(&self, kind: &str, value: &impl Serialize) -> io::Result<()> {
        self.put_raw(kind, &serde_json::to_vec(value)?)
    }
    pub(crate) fn put_raw(&self, kind: &str, bytes: &[u8]) -> io::Result<()> {
        self.scalar(kind)?;
        let bytes = bytes.to_vec();
        self.change(kind, bytes.len(), move |rows, family| rows.put_raw(family, "value", &bytes))
    }
    pub(crate) fn create(&self, kind: &str, bytes: &[u8]) -> io::Result<bool> {
        self.scalar(kind)?;
        let bytes = bytes.to_vec();
        self.change(kind, bytes.len(), move |rows, family| {
            if rows.get_raw(family, "value")?.is_some() {
                return Ok(false);
            }
            rows.put_raw(family, "value", &bytes)?;
            Ok(true)
        })
    }
    pub(crate) fn delete(&self, kind: &str) -> io::Result<()> {
        self.scalar(kind)?;
        self.change(kind, 64, |rows, family| rows.delete(family, "value").map(|_| ()))
    }
    pub(crate) fn page<T: DeserializeOwned>(&self, kind: &str, cursor: &mut u64) -> io::Result<Vec<T>> {
        let page = self.read(kind, |rows, family| rows.page(family, *cursor, 128))?;
        if let Some((last, _)) = page.last() {
            *cursor = *last;
        }
        Ok(page.into_iter().map(|(_, value)| value).collect())
    }
    pub(crate) fn all<T: DeserializeOwned>(&self, kind: &str) -> io::Result<Vec<T>> {
        let mut result = Vec::new();
        let mut cursor = 0;
        loop {
            let page = self.page(kind, &mut cursor)?;
            if page.is_empty() {
                break;
            }
            result.extend(page);
        }
        Ok(result)
    }
    pub(crate) fn version(&self, kind: &str) -> io::Result<u64> {
        use fastctx_storage::rusqlite::OptionalExtension;
        self.read(kind, |rows, family| {
            Ok(rows
                .connection()
                .query_row(
                    "SELECT version FROM conversation_record_versions WHERE conversation=?1 AND family=?2",
                    (rows.conversation(), family),
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0) as u64)
        })
    }
    /// Called while artifact deletion owns the disk gate, before deleting the
    /// supervisor record. Small transactions keep unrelated writers moving.
    pub(crate) fn delete_all(&self) -> io::Result<()> {
        loop {
            let prefix = self.prefix.clone();
            let count = records::write_at(&self.directory, Priority::Background, 4096, move |rows| {
                Ok(rows.connection().execute("DELETE FROM conversation_records WHERE position IN (SELECT position FROM conversation_records WHERE conversation=?1 AND family>=?2 AND family<?3 LIMIT 64)",(rows.conversation(),&prefix,format!("{prefix}\u{10ffff}")))?)
            })?;
            if count == 0 {
                break;
            }
        }
        let prefix = self.prefix.clone();
        records::write_at(&self.directory, Priority::Background, 256, move |rows| {
            for table in ["conversation_record_imports", "conversation_record_versions"] {
                rows.connection().execute(
                    &format!("DELETE FROM {table} WHERE conversation=?1 AND family>=?2 AND family<?3"),
                    (rows.conversation(), &prefix, format!("{prefix}\u{10ffff}")),
                )?;
            }
            Ok(())
        })
    }
}

pub(crate) fn get<T: DeserializeOwned>(marker: &Path) -> io::Result<Option<T>> {
    Job::open(&marker.with_extension("json"))?
        .get(marker.extension().and_then(|s| s.to_str()).ok_or_else(|| io::Error::other("job metadata kind missing"))?)
}
pub(crate) fn raw(marker: &Path) -> io::Result<Option<Vec<u8>>> {
    Job::open(&marker.with_extension("json"))?
        .raw(marker.extension().and_then(|s| s.to_str()).ok_or_else(|| io::Error::other("job metadata kind missing"))?)
}
pub(crate) fn exists(marker: &Path) -> io::Result<bool> {
    raw(marker).map(|bytes| bytes.is_some())
}
pub(crate) fn put(marker: &Path, value: &impl Serialize) -> io::Result<()> {
    put_raw(marker, &serde_json::to_vec(value)?)
}
pub(crate) fn put_raw(marker: &Path, bytes: &[u8]) -> io::Result<()> {
    Job::open(&marker.with_extension("json"))?.put_raw(
        marker.extension().and_then(|s| s.to_str()).ok_or_else(|| io::Error::other("job metadata kind missing"))?,
        bytes,
    )
}
pub(crate) fn delete(marker: &Path) -> io::Result<()> {
    Job::open(&marker.with_extension("json"))?.delete(
        marker.extension().and_then(|s| s.to_str()).ok_or_else(|| io::Error::other("job metadata kind missing"))?,
    )
}

pub(crate) fn read(marker: &Path) -> io::Result<Vec<u8>> {
    raw(marker)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "job metadata is absent"))
}
pub(crate) fn text(marker: &Path) -> io::Result<String> {
    String::from_utf8(read(marker)?).map_err(io::Error::other)
}
pub(crate) fn create(marker: &Path, bytes: &[u8]) -> io::Result<()> {
    let kind =
        marker.extension().and_then(|s| s.to_str()).ok_or_else(|| io::Error::other("job metadata kind missing"))?;
    if Job::open(&marker.with_extension("json"))?.create(kind, bytes)? {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "job metadata already exists"))
    }
}
