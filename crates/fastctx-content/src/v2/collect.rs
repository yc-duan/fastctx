//! Collecting objects nothing holds, and settling at start what a stop left
//! half done (§4.3 I2, fff-04).
//!
//! An object goes in three steps, each after the one before is durable: its
//! row becomes a tombstone in one transaction that finds no frame, index
//! reference, hold or live lease on it and its grace over; its bytes are
//! deleted; its row goes. A tombstone is never present again (I5), so a
//! collector that stopped between the steps deletes, when it starts again,
//! the bytes of that life alone.

use super::Library;
use super::objects::{INCOMING, TOMBSTONE, hex, io, now};
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};
use std::path::Path;
use std::time::Duration;

/// How long after an object was put, or last referred to, it is kept.
const GRACE: Duration = Duration::from_secs(24 * 60 * 60);
/// The most objects one pass makes tombstones of.
const PASS: i64 = 512;
/// How long a published file is checked for at start: a rename the power
/// took is lost within moments of it.
const PUBLISHED_KEPT: Duration = Duration::from_secs(60 * 60);

/// What one collection pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    pub objects: usize,
    pub bytes: u64,
    pub leases: usize,
}

/// What settling at start found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Started {
    /// Tombstones whose bytes a stopped collector left.
    pub tombstones: usize,
    /// Files of no life, or of a life that is not present or preparing.
    pub orphans: usize,
    /// Half-written puts and transfers no transfer row names.
    pub incoming: usize,
    /// Objects published as present whose files are not there: preparing
    /// again, and wanted.
    pub lost: usize,
}

impl Library {
    /// Makes tombstones of up to `PASS` objects nothing holds, deletes their
    /// bytes and rows, and drops expired leases.
    pub fn collect(&self) -> Result<Collected> {
        let cutoff = now() - GRACE.as_millis() as i64;
        let (doomed, leases) = self.database().write_now_at(Priority::Background, 1024, move |db| {
            let leases = db.prepare_cached("DELETE FROM leases WHERE expires < ?1")?.execute([now()])?;
            let mut statement = db.prepare_cached(
                "SELECT o.hash, o.life, o.size FROM objects o
                 WHERE o.state = 1 AND o.placed < ?1 AND (o.unreferenced IS NULL OR o.unreferenced < ?1)
                   AND NOT EXISTS (SELECT 1 FROM frame_objects f WHERE f.hash = o.hash)
                   AND NOT EXISTS (SELECT 1 FROM object_refs r WHERE r.hash = o.hash AND r.count > 0)
                   AND NOT EXISTS (SELECT 1 FROM object_holds h WHERE h.hash = o.hash)
                   AND NOT EXISTS (SELECT 1 FROM leases l WHERE l.hash = o.hash AND l.expires >= ?2)
                 LIMIT ?3",
            )?;
            let doomed = statement
                .query_map(params![cutoff, now(), PASS], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)? as u64))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut tombstone = db.prepare_cached("UPDATE objects SET state = 2 WHERE hash = ?1 AND life = ?2")?;
            for (hash, life, _) in &doomed {
                tombstone.execute(params![hash, life])?;
            }
            Ok((doomed, leases))
        })?;
        let mut collected = Collected { leases, ..Collected::default() };
        for (hash, life, size) in doomed {
            let hash: [u8; 32] = hash.try_into().map_err(|_| crate::Error::Invalid("a malformed object hash".into()))?;
            self.delete_life(&hash, life, size)?;
            collected.objects += 1;
            collected.bytes += size;
        }
        Ok(collected)
    }

    /// Deletes the bytes of a tombstone's life, then its row, and the chunk
    /// manifest once no life of the hash is left.
    fn delete_life(&self, hash: &[u8; 32], life: i64, size: u64) -> Result<()> {
        if size as usize <= super::ROW_BYTES {
            let hash = *hash;
            self.0.bytes.write_relaxed_now_at(Priority::Background, 64, move |db| {
                db.prepare_cached("DELETE FROM object_bytes WHERE hash = ?1 AND life = ?2")?
                    .execute(params![&hash[..], life])?;
                Ok(())
            })?;
            self.0.bytes.settle_now(Priority::Background)?;
        } else {
            fastctx_platform::durable_file::delete_unreferenced(&self.object_file(hash, life)).map_err(io)?;
        }
        let hash = *hash;
        self.database().write_now_at(Priority::Background, 64, move |db| {
            db.prepare_cached("DELETE FROM objects WHERE hash = ?1 AND life = ?2 AND state = 2")?
                .execute(params![&hash[..], life])?;
            db.prepare_cached(
                "DELETE FROM object_chunks WHERE hash = ?1 AND NOT EXISTS (SELECT 1 FROM objects WHERE hash = ?1)",
            )?
            .execute([&hash[..]])?;
            db.prepare_cached("DELETE FROM published WHERE hash = ?1 AND life = ?2")?.execute(params![&hash[..], life])?;
            Ok(())
        })?;
        Ok(())
    }

    /// Settles what a stop left before objects are served: the bytes of
    /// tombstones, files no life owns, half-written incoming files, and
    /// recently published files the power took.
    pub(super) fn startup(&self) -> Result<Started> {
        let mut started = Started::default();
        let tombstones: Vec<(Vec<u8>, i64, i64)> = self.database().read_now(|db| {
            let mut statement = db.prepare_cached("SELECT hash, life, size FROM objects WHERE state = ?1")?;
            let rows = statement.query_map([TOMBSTONE], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        for (hash, life, size) in tombstones {
            let hash: [u8; 32] = hash.try_into().map_err(|_| crate::Error::Invalid("a malformed object hash".into()))?;
            self.delete_life(&hash, life, size as u64)?;
            started.tombstones += 1;
        }
        started.lost = self.check_published()?;
        started.orphans = self.remove_orphans()?;
        started.incoming = self.remove_incoming()?;
        self.remove_orphan_bytes()?;
        Ok(started)
    }

    /// Published files that are not there go back to preparing and are
    /// wanted again; the record of what was published lasts an hour.
    fn check_published(&self) -> Result<usize> {
        let published: Vec<(Vec<u8>, i64, i64)> = self.database().read_now(|db| {
            let mut statement = db.prepare_cached(
                "SELECT p.hash, p.life, o.size FROM published p JOIN objects o ON o.hash = p.hash AND o.life = p.life
                 WHERE o.state = 1",
            )?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        let mut lost = Vec::new();
        for (hash, life, size) in published {
            let Ok(key) = <[u8; 32]>::try_from(hash.as_slice()) else { continue };
            let file = self.object_file(&key, life);
            let whole = std::fs::metadata(&file).is_ok_and(|meta| meta.len() == size as u64);
            if !whole {
                lost.push((hash, life, size));
            }
        }
        let count = lost.len();
        let kept = now() - PUBLISHED_KEPT.as_millis() as i64;
        self.database().write_now_at(Priority::Normal, 256 + count * 64, move |db| {
            for (hash, life, size) in &lost {
                db.prepare_cached("UPDATE objects SET state = 0 WHERE hash = ?1 AND life = ?2 AND state = 1")?
                    .execute(params![hash, life])?;
                db.prepare_cached(
                    "INSERT OR IGNORE INTO wanted (hash, size, role, priority, hint) VALUES (?1, ?2, 'lost', 0, NULL)",
                )?
                .execute(params![hash, size])?;
            }
            db.prepare_cached("DELETE FROM published WHERE at < ?1")?.execute([kept])?;
            Ok(())
        })?;
        Ok(count)
    }

    /// Files in the objects' buckets that are no present or preparing life.
    fn remove_orphans(&self) -> Result<usize> {
        let mut removed = 0;
        let buckets = match std::fs::read_dir(&self.0.objects) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(io(error)),
        };
        for bucket in buckets {
            let bucket = bucket.map_err(io)?;
            if bucket.file_name() == INCOMING || !bucket.file_type().map_err(io)?.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(bucket.path()).map_err(io)? {
                let path = entry.map_err(io)?.path();
                if !self.owned(&path)? {
                    fastctx_platform::durable_file::delete_unreferenced(&path).map_err(io)?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    /// Whether `path` is the file of a present or preparing life.
    fn owned(&self, path: &Path) -> Result<bool> {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { return Ok(false) };
        let Some((name, life)) = name.split_once('.') else { return Ok(false) };
        let (Ok(life), Some(hash)) = (life.parse::<i64>(), parse_hex(name)) else { return Ok(false) };
        if hex(&hash) != name {
            return Ok(false);
        }
        Ok(self.database().read_now(move |db| {
            Ok(db
                .prepare_cached("SELECT 1 FROM objects WHERE hash = ?1 AND life = ?2 AND state IN (0, 1)")?
                .query_row(params![&hash[..], life], |_| Ok(()))
                .optional()?
                .is_some())
        })?)
    }

    /// Incoming files no transfer row names: puts a stop cut short.
    fn remove_incoming(&self) -> Result<usize> {
        let directory = self.0.objects.join(INCOMING);
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(io(error)),
        };
        let parts: std::collections::HashSet<String> = self.database().read_now(|db| {
            let mut statement = db.prepare_cached("SELECT part FROM transfers")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?;
        let mut removed = 0;
        for entry in entries {
            let entry = entry.map_err(io)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !parts.contains(&name) {
                fastctx_platform::durable_file::delete_unreferenced(&entry.path()).map_err(io)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Small objects' bytes of no present or preparing life.
    fn remove_orphan_bytes(&self) -> Result<()> {
        let rows: Vec<(Vec<u8>, i64)> = self.0.bytes.read_now(|db| {
            let mut statement = db.prepare_cached("SELECT hash, life FROM object_bytes")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        let mut orphans = Vec::new();
        for (hash, life) in rows {
            let known = {
                let hash = hash.clone();
                self.database().read_now(move |db| {
                    Ok(db
                        .prepare_cached("SELECT 1 FROM objects WHERE hash = ?1 AND life = ?2 AND state IN (0, 1)")?
                        .query_row(params![hash, life], |_| Ok(()))
                        .optional()?
                        .is_some())
                })?
            };
            if !known {
                orphans.push((hash, life));
            }
        }
        if orphans.is_empty() {
            return Ok(());
        }
        self.0.bytes.write_now_at(Priority::Background, 64 * orphans.len(), move |db| {
            for (hash, life) in &orphans {
                db.prepare_cached("DELETE FROM object_bytes WHERE hash = ?1 AND life = ?2")?.execute(params![hash, life])?;
            }
            Ok(())
        })?;
        Ok(())
    }
}

fn parse_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut hash = [0u8; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(hash)
}
