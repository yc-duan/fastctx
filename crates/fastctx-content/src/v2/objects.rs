//! Objects: bytes kept by their SHA-256 and length (§4.3).
//!
//! An object's row says whether its bytes are there: `present` only once
//! they are durable, a file synced and renamed and its directory synced, or
//! a row of the bytes database committed in full (I1). Whoever is about to
//! refer to an object holds a lease on it, made in the same transaction that
//! finds it present or makes it so, and let go with the commit that refers
//! to it (I4); collection looks for leases, references and holds in that
//! same database, so nothing it collects is about to be referred to (I2).
//! A hash that comes back after its bytes went is a new life (I5).

use super::Library;
use crate::error::invalid;
use crate::record::ObjectRef;
use crate::{Error, Result};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// An object up to this size is a row of the bytes database; a larger one
/// is a file with a manifest of its chunks.
pub const ROW_BYTES: usize = 64 * 1024;
/// A large object's chunk: the unit transfers ask for and verify.
pub const CHUNK_BYTES: u64 = 1024 * 1024;
/// How long a lease holds an object when its holder says nothing else.
pub const LEASE: Duration = Duration::from_secs(24 * 60 * 60);
/// The most chunk digests one manifest row holds.
pub(super) const DIGESTS_PER_ROW: u64 = 4096;
/// The most one read gives.
pub const READ_BYTES: u32 = 1024 * 1024;

/// An object row's states: preparing 0, present 1, tombstone 2.
pub(super) const TOMBSTONE: i64 = 2;

/// The directory, within the objects', of files being written.
pub(super) const INCOMING: &str = "incoming";

static WRITING: AtomicU64 = AtomicU64::new(0);

/// A hold on an object until the commit that refers to it lets it go, or it
/// expires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub id: i64,
    pub object: ObjectRef,
}

pub(super) fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64)
}

pub(super) fn io(error: std::io::Error) -> Error {
    Error::Storage(fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Unavailable, error.to_string()))
}

pub(super) fn hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The present life of `hash`, if any, and its length.
pub(super) fn present(db: &Connection, hash: &[u8; 32]) -> fastctx_storage::Result<Option<(i64, u64)>> {
    Ok(db
        .prepare_cached("SELECT life, size FROM objects WHERE hash = ?1 AND state = 1")?
        .query_row([&hash[..]], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as u64)))
        .optional()?)
}

pub(super) fn lease_in(db: &Connection, object: ObjectRef, holder: &str, ttl: Duration) -> fastctx_storage::Result<Lease> {
    let expires = now() + ttl.as_millis() as i64;
    db.prepare_cached("INSERT INTO leases (hash, holder, expires) VALUES (?1, ?2, ?3)")?
        .execute(params![&object.hash[..], holder, expires])?;
    Ok(Lease { id: db.last_insert_rowid(), object })
}

/// Lets go of leases as part of the commit that refers to their objects.
pub(crate) fn release_in(db: &Connection, leases: &[Lease]) -> fastctx_storage::Result<()> {
    let mut statement = db.prepare_cached("DELETE FROM leases WHERE id = ?1")?;
    for lease in leases {
        statement.execute([lease.id])?;
    }
    Ok(())
}

/// A life no object here ever had: lives are numbered across every object
/// and never given again (§4.3 I5), so whatever deletes an old life's bytes,
/// however late, never meets a new life's.
pub(super) fn next_life(db: &Connection) -> fastctx_storage::Result<i64> {
    let last: Option<i64> = db.prepare_cached("SELECT value FROM stamps WHERE name = 'life'")?.query_row([], |row| row.get(0)).optional()?;
    let next = match last {
        Some(last) => last + 1,
        // Past every life numbered before lives were counted.
        None => db.prepare_cached("SELECT COALESCE(MAX(life), 0) + 1 FROM objects")?.query_row([], |row| row.get(0))?,
    };
    db.prepare_cached("INSERT INTO stamps (name, value) VALUES ('life', ?1) ON CONFLICT (name) DO UPDATE SET value = excluded.value")?
        .execute([next])?;
    Ok(next)
}

/// A new life of `hash`, preparing: past every life any object had,
/// collected or not.
fn new_life(db: &Connection, object: ObjectRef, chunk: u64) -> fastctx_storage::Result<i64> {
    let life = next_life(db)?;
    db.prepare_cached(
        "INSERT INTO objects (hash, life, size, state, placed, unreferenced, chunk) VALUES (?1, ?2, ?3, 0, ?4, NULL, ?5)",
    )?
    .execute(params![&object.hash[..], life, object.size as i64, now(), chunk as i64])?;
    Ok(life)
}

/// What one put found or made.
enum Placed {
    /// The object was present already: leased, and nothing written.
    Here(Lease),
    /// A new life to write the bytes of.
    Life(i64),
}

impl Library {
    pub(super) fn object_file(&self, hash: &[u8; 32], life: i64) -> PathBuf {
        let name = hex(hash);
        self.0.objects.join(&name[..2]).join(format!("{name}.{life}"))
    }

    /// Leases `object` for `holder` if it is present: the caller is about to
    /// refer to it.
    pub fn lease(&self, object: ObjectRef, holder: &str, ttl: Duration) -> Result<Option<Lease>> {
        let holder = holder.to_string();
        Ok(self.database().write_relaxed_now_at(Priority::Normal, 128, move |db| {
            match present(db, &object.hash)? {
                Some((_, size)) if size == object.size => Ok(Some(lease_in(db, object, &holder, ttl)?)),
                Some(_) => Err(invalid("an object of that hash is here with another length")),
                None => Ok(None),
            }
        })?)
    }

    /// Lets go of a lease whose holder will not refer to its object.
    pub fn release(&self, lease: Lease) -> Result<()> {
        Ok(self.database().write_relaxed_now_at(Priority::Normal, 64, move |db| release_in(db, &[lease]))?)
    }

    /// The length of `hash` when it is present.
    pub fn size(&self, hash: &[u8; 32]) -> Result<Option<u64>> {
        Ok(self.database().read_now(|db| Ok(present(db, hash)?.map(|(_, size)| size)))?)
    }

    /// Keeps what `source` gives as an object, leased to `holder`, its bytes
    /// durable before it is present. Bytes already present are not written
    /// again. Call on a thread that may block.
    pub fn put(&self, source: &mut dyn Read, holder: &str, ttl: Duration) -> Result<Lease> {
        let mut held = Vec::with_capacity(ROW_BYTES + 1);
        let mut window = vec![0u8; 256 * 1024];
        let mut whole = Sha256::new();
        let mut chunk = Sha256::new();
        let mut in_chunk = 0u64;
        let mut digests: Vec<[u8; 32]> = Vec::new();
        let mut spilled: Option<(std::fs::File, PathBuf)> = None;
        let mut size = 0u64;
        loop {
            let read = source.read(&mut window).map_err(io)?;
            if read == 0 {
                break;
            }
            let mut piece = &window[..read];
            whole.update(piece);
            size += read as u64;
            while !piece.is_empty() {
                let take = piece.len().min((CHUNK_BYTES - in_chunk) as usize);
                chunk.update(&piece[..take]);
                in_chunk += take as u64;
                if in_chunk == CHUNK_BYTES {
                    digests.push(std::mem::take(&mut chunk).finalize().into());
                    in_chunk = 0;
                }
                piece = &piece[take..];
            }
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
        if in_chunk > 0 {
            digests.push(chunk.finalize().into());
        }
        let object = ObjectRef { hash: whole.finalize().into(), size };
        let large = spilled.is_some();
        let holder = holder.to_string();
        let leasing = holder.clone();
        let placed = self.database().write_relaxed_now_at(Priority::Normal, 256, move |db| {
            if let Some((_, known)) = present(db, &object.hash)? {
                if known != object.size {
                    return Err(invalid("an object of that hash is here with another length"));
                }
                return Ok(Placed::Here(lease_in(db, object, &leasing, ttl)?));
            }
            Ok(Placed::Life(new_life(db, object, if large { CHUNK_BYTES } else { 0 })?))
        })?;
        let life = match placed {
            Placed::Here(lease) => {
                if let Some((file, incoming)) = spilled {
                    drop(file);
                    let _ = std::fs::remove_file(incoming);
                }
                return Ok(lease);
            }
            Placed::Life(life) => life,
        };
        match spilled {
            Some((file, incoming)) => {
                file.sync_all().map_err(io)?;
                drop(file);
                let place = self.object_file(&object.hash, life);
                fastctx_platform::durable_file::prepare_directory(place.parent().expect("an object's bucket"))
                    .map_err(io)?;
                std::fs::rename(&incoming, &place).map_err(io)?;
                sync_parent(&place).map_err(io)?;
            }
            None => {
                let bytes = held;
                self.0.bytes.write_now_at(Priority::Normal, bytes.len() + 64, move |db| {
                    db.prepare_cached("INSERT OR REPLACE INTO object_bytes (hash, life, bytes) VALUES (?1, ?2, ?3)")?
                        .execute(params![&object.hash[..], life, bytes])?;
                    Ok(())
                })?;
            }
        }
        let bytes = 256 + digests.len() * 32;
        Ok(self.database().write_now_at(Priority::Normal, bytes, move |db| {
            let changed = db
                .prepare_cached("UPDATE objects SET state = 1, placed = ?3 WHERE hash = ?1 AND life = ?2 AND state = 0")?
                .execute(params![&object.hash[..], life, now()])?;
            if changed != 1 {
                return Err(invalid("the object's life was settled by someone else while its bytes were written"));
            }
            if large {
                db.prepare_cached("INSERT OR REPLACE INTO published (hash, life, at) VALUES (?1, ?2, ?3)")?
                    .execute(params![&object.hash[..], life, now()])?;
                let mut manifest = db.prepare_cached(
                    "INSERT OR REPLACE INTO object_chunks (hash, first, digests) VALUES (?1, ?2, ?3)",
                )?;
                for (row, digests) in digests.chunks(DIGESTS_PER_ROW as usize).enumerate() {
                    let joined: Vec<u8> = digests.iter().flatten().copied().collect();
                    manifest.execute(params![&object.hash[..], row as i64 * DIGESTS_PER_ROW as i64, joined])?;
                }
            }
            lease_in(db, object, &holder, ttl)
        })?)
    }

    /// Up to `READ_BYTES` of the present object `hash` from `offset`.
    pub fn read(&self, hash: &[u8; 32], offset: u64, length: u32) -> Result<Vec<u8>> {
        let Some((life, size)) = self.database().read_now(|db| present(db, hash))? else {
            return Err(Error::Invalid("the object is not here".into()));
        };
        let length = u64::from(length.min(READ_BYTES)).min(size.saturating_sub(offset));
        if size as usize <= ROW_BYTES {
            let hash = *hash;
            let bytes: Option<Vec<u8>> = self.0.bytes.read_now(move |db| {
                Ok(db
                    .prepare_cached("SELECT bytes FROM object_bytes WHERE hash = ?1 AND life = ?2")?
                    .query_row(params![&hash[..], life], |row| row.get(0))
                    .optional()?)
            })?;
            let bytes = bytes.ok_or_else(|| Error::Invalid("the object's bytes are missing".into()))?;
            let start = (offset as usize).min(bytes.len());
            let end = (start + length as usize).min(bytes.len());
            return Ok(bytes[start..end].to_vec());
        }
        let mut file = std::fs::File::open(self.object_file(hash, life)).map_err(io)?;
        file.seek(SeekFrom::Start(offset)).map_err(io)?;
        let mut bytes = vec![0u8; length as usize];
        file.read_exact(&mut bytes).map_err(io)?;
        Ok(bytes)
    }

    /// The chunk digests of a large present object, in order.
    pub fn chunks(&self, hash: &[u8; 32]) -> Result<Vec<[u8; 32]>> {
        let hash = *hash;
        Ok(self.database().read_now(move |db| {
            let mut statement =
                db.prepare_cached("SELECT digests FROM object_chunks WHERE hash = ?1 ORDER BY first")?;
            let rows = statement.query_map([&hash[..]], |row| row.get::<_, Vec<u8>>(0))?;
            let mut digests = Vec::new();
            for row in rows {
                for digest in row?.chunks_exact(32) {
                    digests.push(digest.try_into().expect("32 bytes"));
                }
            }
            Ok(digests)
        })?)
    }

    fn incoming(&self) -> Result<(std::fs::File, PathBuf)> {
        let directory = self.0.objects.join(INCOMING);
        std::fs::create_dir_all(&directory).map_err(io)?;
        let name = format!("put-{}-{}.part", std::process::id(), WRITING.fetch_add(1, Ordering::Relaxed));
        let path = directory.join(name);
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(io)?;
        Ok((file, path))
    }
}

/// Syncs the directory a file was renamed into, where the system can.
pub(super) fn sync_parent(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(path.parent().ok_or_else(|| std::io::Error::other("an object has no directory"))?)?
            .sync_all()
    }
    #[cfg(not(unix))]
    {
        // Windows has no documented flush of a directory entry; a rename
        // lost with the power is found at the next start (`published`).
        let _ = path;
        Ok(())
    }
}
