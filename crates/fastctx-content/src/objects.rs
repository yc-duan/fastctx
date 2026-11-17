//! Objects: large bytes kept by their content. A record refers to an object
//! by its SHA-256 and length; the bytes are here once however many records
//! and scopes refer to them, and are read a range at a time.
//!
//! Small objects are rows of their own database, so the content database's
//! pages hold items only. Larger ones are a file each.
//!
//! An object is put before the record that refers to it is appended, and
//! goes once no record refers to it (`crate::reclaim`). Putting bytes that
//! are here already marks them put now, and an object is not reclaimed
//! until it was put a while ago: so a record is appended within that while
//! of putting what it refers to.

use crate::record::ObjectRef;
use crate::{Error, Result};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_storage::{Database, Priority};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The objects' database within the directory the content is opened in.
pub const OBJECTS_FILE: &str = "objects.sqlite3";
/// The directory of the objects that are files.
pub const OBJECTS_DIRECTORY: &str = "objects";
/// An object up to this size is a row; a larger one is a file.
pub const ROW_BYTES: usize = 64 * 1024;
/// The most one read gives.
pub const READ_BYTES: u32 = 1024 * 1024;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS objects (
    hash BLOB PRIMARY KEY,
    size INTEGER NOT NULL,
    made INTEGER NOT NULL,
    bytes BLOB
) WITHOUT ROWID;
";
/// The directory, within the objects', of files being written.
const INCOMING_DIRECTORY: &str = "incoming";

static INCOMING: AtomicU64 = AtomicU64::new(0);

pub struct Objects {
    database: Database,
    directory: PathBuf,
}

fn io(error: std::io::Error) -> Error {
    Error::Storage(fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Unavailable, error.to_string()))
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64)
}

fn hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl Objects {
    pub(crate) fn open(directory: &Path) -> Result<Self> {
        let database = Database::open(&directory.join(OBJECTS_FILE), SCHEMA)?;
        Ok(Self { database, directory: directory.join(OBJECTS_DIRECTORY) })
    }

    fn file(&self, hash: &[u8; 32]) -> PathBuf {
        let name = hex(hash);
        self.directory.join(&name[..2]).join(name)
    }

    /// The length of an object that is here.
    pub fn size(&self, hash: &[u8; 32]) -> Result<Option<u64>> {
        Ok(self.database.read_now(|db| {
            Ok(db
                .prepare_cached("SELECT size FROM objects WHERE hash = ?1")?
                .query_row([&hash[..]], |row| row.get::<_, i64>(0))
                .optional()?
                .map(|size| size as u64))
        })?)
    }

    /// Keeps `bytes` as an object. Bytes already here are not written again.
    /// Call on a thread that may block.
    pub fn put(&self, bytes: &[u8]) -> Result<ObjectRef> {
        self.put_from(&mut std::io::Cursor::new(bytes))
    }

    /// Keeps what `source` gives as an object, a window of it in memory at a
    /// time. Call on a thread that may block.
    pub fn put_from(&self, source: &mut dyn Read) -> Result<ObjectRef> {
        let mut hasher = Sha256::new();
        let mut held = Vec::with_capacity(ROW_BYTES + 1);
        let mut window = vec![0u8; ROW_BYTES];
        let mut spilled: Option<(std::fs::File, PathBuf)> = None;
        let mut size = 0u64;
        loop {
            let read = source.read(&mut window).map_err(io)?;
            if read == 0 {
                break;
            }
            hasher.update(&window[..read]);
            size += read as u64;
            match &mut spilled {
                Some((file, _)) => file.write_all(&window[..read]).map_err(io)?,
                None if held.len() + read <= ROW_BYTES => held.extend_from_slice(&window[..read]),
                None => {
                    let (mut file, path) = self.incoming()?;
                    file.write_all(&held).map_err(io)?;
                    file.write_all(&window[..read]).map_err(io)?;
                    held = Vec::new();
                    spilled = Some((file, path));
                }
            }
        }
        let object = ObjectRef { hash: hasher.finalize().into(), size };
        // Marked put now where it is here, which keeps it from being
        // reclaimed under the record about to refer to it. The mark does
        // not wait for the disk: lost with the power, so is that record.
        let here = self.database.write_relaxed_now_at(Priority::Normal, 64, move |db| {
            Ok(db
                .prepare_cached("UPDATE objects SET made = ?2 WHERE hash = ?1")?
                .execute(params![&object.hash[..], now()])?
                > 0)
        })?;
        let bytes = match spilled {
            Some((file, incoming)) => {
                if here {
                    drop(file);
                    let _ = std::fs::remove_file(&incoming);
                    return Ok(object);
                }
                file.sync_all().map_err(io)?;
                drop(file);
                let place = self.file(&object.hash);
                std::fs::create_dir_all(place.parent().expect("an object's directory")).map_err(io)?;
                // The same bytes may be put twice at once: whichever is
                // renamed second finds them there.
                if std::fs::rename(&incoming, &place).is_err() && !place.exists() {
                    return Err(io(std::io::Error::other("an object could not be put in its place")));
                }
                let _ = std::fs::remove_file(&incoming);
                None
            }
            None if here => return Ok(object),
            None => Some(held),
        };
        let retained = bytes.as_ref().map_or(0, Vec::len) + 64;
        self.database.write_now_at(Priority::Normal, retained, move |db| {
            db.prepare_cached("INSERT OR IGNORE INTO objects (hash, size, made, bytes) VALUES (?1, ?2, ?3, ?4)")?
                .execute(params![&object.hash[..], object.size as i64, now(), bytes])?;
            Ok(())
        })?;
        Ok(object)
    }

    fn incoming(&self) -> Result<(std::fs::File, PathBuf)> {
        let directory = self.directory.join(INCOMING_DIRECTORY);
        std::fs::create_dir_all(&directory).map_err(io)?;
        let name = format!("{}-{}-{}", std::process::id(), now(), INCOMING.fetch_add(1, Ordering::Relaxed));
        let path = directory.join(name);
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(io)?;
        Ok((file, path))
    }

    /// At most `length` bytes of an object from `offset`, `READ_BYTES` at
    /// most; none when the object is not here. Call on a thread that may block.
    pub fn read(&self, hash: &[u8; 32], offset: u64, length: u32) -> Result<Option<Vec<u8>>> {
        let length = u64::from(length.min(READ_BYTES));
        let stored: Option<(i64, Option<Vec<u8>>)> = self.database.read_now(|db| {
            Ok(db
                .prepare_cached("SELECT size, substr(bytes, ?2, ?3) FROM objects WHERE hash = ?1")?
                .query_row(params![&hash[..], offset as i64 + 1, length as i64], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?)
        })?;
        let Some((size, row)) = stored else { return Ok(None) };
        if let Some(bytes) = row {
            return Ok(Some(bytes));
        }
        let size = size as u64;
        if offset >= size {
            return Ok(Some(Vec::new()));
        }
        let mut file = std::fs::File::open(self.file(hash)).map_err(io)?;
        file.seek(SeekFrom::Start(offset)).map_err(io)?;
        let mut bytes = vec![0u8; length.min(size - offset) as usize];
        file.read_exact(&mut bytes).map_err(io)?;
        Ok(Some(bytes))
    }

    /// Removes an object, once nothing refers to it. Call on a thread that
    /// may block.
    pub fn remove(&self, hash: &[u8; 32]) -> Result<()> {
        let hash = *hash;
        self.database.write_now_at(Priority::Background, 64, move |db| {
            db.prepare_cached("DELETE FROM objects WHERE hash = ?1")?.execute([&hash[..]])?;
            Ok(())
        })?;
        match std::fs::remove_file(self.file(&hash)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(io(error)),
            _ => Ok(()),
        }
    }

    /// The objects after `after`, in the order of their hashes, that were
    /// put before `before`: `most` at most.
    pub(crate) fn aged(&self, after: &[u8], before: i64, most: usize) -> Result<Vec<[u8; 32]>> {
        Ok(self.database.read_now(|db| {
            let mut aged = Vec::new();
            let mut rows =
                db.prepare_cached("SELECT hash FROM objects WHERE hash > ?1 AND made < ?2 ORDER BY hash LIMIT ?3")?;
            let mut rows = rows.query(params![after, before, most as i64])?;
            while let Some(row) = rows.next()? {
                if let Ok(hash) = <[u8; 32]>::try_from(row.get::<_, Vec<u8>>(0)?) {
                    aged.push(hash);
                }
            }
            Ok(aged)
        })?)
    }

    /// Removes those of `hashes` still put before `before`, with their
    /// files, and says how many went and how many bytes they were. One put
    /// again meanwhile stays. Call on a thread that may block.
    pub(crate) fn drop_aged(&self, hashes: Vec<[u8; 32]>, before: i64) -> Result<(usize, u64)> {
        let files: Vec<PathBuf> = hashes.iter().map(|hash| self.file(hash)).collect();
        Ok(self.database.write_now_at(Priority::Background, 64 * hashes.len(), move |db| {
            let mut gone =
                db.prepare_cached("DELETE FROM objects WHERE hash = ?1 AND made < ?2 RETURNING size, bytes IS NULL")?;
            let (mut removed, mut bytes) = (0, 0u64);
            for (hash, file) in hashes.iter().zip(&files) {
                let dropped: Option<(i64, bool)> =
                    gone.query_row(params![&hash[..], before], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
                let Some((size, filed)) = dropped else { continue };
                // The file goes with its row, in the one writer: a put of
                // the same bytes after this finds neither and keeps both.
                if filed {
                    let _ = std::fs::remove_file(file);
                }
                removed += 1;
                bytes += size as u64;
            }
            Ok((removed, bytes))
        })?)
    }

    /// Removes what an interrupted put left: files being written, and files
    /// in place without a row, in one of the 256 directories each time
    /// (`turn`), that were last written before `before`. Call on a thread
    /// that may block.
    pub(crate) fn sweep(&self, turn: u8, before: SystemTime) -> Result<usize> {
        let old = |entry: &std::fs::DirEntry| {
            entry.metadata().and_then(|facts| facts.modified()).is_ok_and(|written| written < before)
        };
        let listed = |directory: PathBuf| -> Vec<std::fs::DirEntry> {
            std::fs::read_dir(directory).map(|entries| entries.flatten().collect()).unwrap_or_default()
        };
        let mut removed = 0;
        for entry in listed(self.directory.join(INCOMING_DIRECTORY)) {
            if old(&entry) && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        for entry in listed(self.directory.join(format!("{turn:02x}"))) {
            let named = entry.file_name();
            let Some(hash) = named.to_str().and_then(unhex) else { continue };
            if old(&entry) && self.size(&hash)?.is_none() && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

fn unhex(name: &str) -> Option<[u8; 32]> {
    if name.len() != 64 || !name.is_ascii() {
        return None;
    }
    let mut hash = [0u8; 32];
    for (byte, pair) in hash.iter_mut().zip(name.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(hash)
}
