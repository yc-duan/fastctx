//! Receiving a large object a chunk at a time, across breaks and restarts
//! (§4.3 接收续传, fff-14, R04).
//!
//! Every chunk of one transfer goes to its offset in one part file, checked
//! against the object's chunk manifest before it counts. Chunks are made
//! durable before they are recorded: the part file is synced every 16
//! chunks or 100 ms, and only then are their ranges committed, so a range on
//! record always has its bytes on disk; a record lost asks for a few chunks
//! again. At start a range past the part file's end is cut back to it. Once
//! every chunk is there the whole file is hashed, synced, renamed into place
//! and published as the object's new life.

use super::Library;
use super::objects::{CHUNK_BYTES, INCOMING, Lease, io, now, present, sync_parent};
use crate::error::invalid;
use crate::record::ObjectRef;
use crate::{Error, Result};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Chunks written before the part file is synced and their ranges recorded.
const SYNC_CHUNKS: usize = 16;
/// The longest a written chunk waits to be synced and recorded.
const SYNC_AFTER: Duration = Duration::from_millis(100);

/// One object being received.
pub struct Transfer {
    library: Library,
    pub id: i64,
    pub object: ObjectRef,
    digests: Vec<[u8; 32]>,
    file: std::fs::File,
    part: PathBuf,
    /// Chunk numbers whose bytes are durable and recorded.
    durable: Vec<bool>,
    /// Chunks written since the last sync.
    written: Vec<u64>,
    since: Instant,
}

/// Chunk numbers as ranges, `[first, end)`, encoded eight bytes each end.
fn encode(durable: &[bool]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut index = 0;
    while index < durable.len() {
        if !durable[index] {
            index += 1;
            continue;
        }
        let start = index;
        while index < durable.len() && durable[index] {
            index += 1;
        }
        bytes.extend_from_slice(&(start as u64).to_le_bytes());
        bytes.extend_from_slice(&(index as u64).to_le_bytes());
    }
    bytes
}

fn decode(bytes: &[u8], chunks: usize) -> Vec<bool> {
    let mut durable = vec![false; chunks];
    for pair in bytes.chunks_exact(16) {
        let start = u64::from_le_bytes(pair[..8].try_into().expect("eight bytes")) as usize;
        let end = u64::from_le_bytes(pair[8..].try_into().expect("eight bytes")) as usize;
        for flag in durable.iter_mut().take(end.min(chunks)).skip(start) {
            *flag = true;
        }
    }
    durable
}

fn chunk_count(size: u64) -> usize {
    size.div_ceil(CHUNK_BYTES) as usize
}

impl Library {
    /// Starts receiving `object`, whose chunks hash to `digests`, or takes up
    /// the transfer of it already under way; `None` when it is present.
    pub fn receive(&self, object: ObjectRef, digests: Vec<[u8; 32]>, source: Option<String>) -> Result<Option<Transfer>> {
        if digests.len() != chunk_count(object.size) || object.size as usize <= super::ROW_BYTES {
            return Err(Error::Invalid("a transfer is of a large object, with one digest a chunk".into()));
        }
        let directory = self.0.objects.join(INCOMING);
        std::fs::create_dir_all(&directory).map_err(io)?;
        let manifest: Vec<u8> = digests.iter().flatten().copied().collect();
        let found = self.database().write_now_at(Priority::Normal, 256 + manifest.len(), move |db| {
            if present(db, &object.hash)?.is_some() {
                return Ok(None);
            }
            let known: Option<(i64, String, Vec<u8>, Vec<u8>)> = db
                .prepare_cached("SELECT id, part, ranges, manifest FROM transfers WHERE hash = ?1 AND size = ?2")?
                .query_row(params![&object.hash[..], object.size as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .optional()?;
            let mut stale = None;
            match known {
                Some((id, part, ranges, known)) if known == manifest => return Ok(Some((id, part, ranges, None))),
                // Another manifest: what was received under the old one
                // counts for nothing.
                Some((id, part, _, _)) => {
                    db.prepare_cached("DELETE FROM transfers WHERE id = ?1")?.execute([id])?;
                    stale = Some(part);
                }
                None => {}
            }
            db.prepare_cached(
                "INSERT INTO transfers (hash, size, chunk, part, ranges, manifest, source) VALUES (?1, ?2, ?3, '', x'', ?4, ?5)",
            )?
            .execute(params![&object.hash[..], object.size as i64, CHUNK_BYTES as i64, manifest, source])?;
            let id = db.last_insert_rowid();
            let part = format!("transfer-{id}.part");
            db.prepare_cached("UPDATE transfers SET part = ?2 WHERE id = ?1")?.execute(params![id, part])?;
            Ok(Some((id, part, Vec::new(), stale)))
        })?;
        let Some((id, part, ranges, stale)) = found else { return Ok(None) };
        if let Some(stale) = stale {
            let _ = std::fs::remove_file(directory.join(stale));
        }
        let path = directory.join(&part);
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).map_err(io)?;
        let chunks = chunk_count(object.size);
        Ok(Some(Transfer {
            library: self.clone(),
            id,
            object,
            digests,
            file,
            part: path,
            durable: decode(&ranges, chunks),
            written: Vec::new(),
            since: Instant::now(),
        }))
    }

    /// The transfer of `object` already under way, taken up under the
    /// manifest it began with; `None` when none is, or the object is here.
    pub fn under_way(&self, object: ObjectRef) -> Result<Option<Transfer>> {
        let manifest: Option<Vec<u8>> = self.database().read_now(move |db| {
            Ok(db
                .prepare_cached("SELECT manifest FROM transfers WHERE hash = ?1 AND size = ?2")?
                .query_row(params![&object.hash[..], object.size as i64], |row| row.get(0))
                .optional()?)
        })?;
        let Some(manifest) = manifest else { return Ok(None) };
        let digests = manifest.chunks_exact(32).map(|digest| digest.try_into().expect("32 bytes")).collect();
        self.receive(object, digests, None)
    }

    /// Cuts every transfer's recorded ranges back to what its part file
    /// holds, and forgets transfers whose part file is gone.
    pub(super) fn settle_transfers(&self) -> Result<usize> {
        let transfers: Vec<(i64, i64, String, Vec<u8>)> = self.database().read_now(|db| {
            let mut statement = db.prepare_cached("SELECT id, size, part, ranges FROM transfers")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        let mut cut = 0;
        for (id, size, part, ranges) in transfers {
            let length = std::fs::metadata(self.0.objects.join(INCOMING).join(&part)).map(|meta| meta.len()).ok();
            let chunks = chunk_count(size as u64);
            let mut durable = decode(&ranges, chunks);
            let whole = length.map_or(0, |length| {
                // A chunk counts only where the file holds all of it.
                (0..chunks).take_while(|index| ((*index as u64 + 1) * CHUNK_BYTES).min(size as u64) <= length).count()
            });
            let mut changed = false;
            for flag in durable.iter_mut().skip(whole) {
                changed |= std::mem::replace(flag, false);
            }
            if length.is_none() {
                self.database().write_now_at(Priority::Normal, 64, move |db| {
                    db.prepare_cached("DELETE FROM transfers WHERE id = ?1")?.execute([id])?;
                    Ok(())
                })?;
                cut += 1;
            } else if changed {
                let ranges = encode(&durable);
                self.database().write_now_at(Priority::Normal, 64 + ranges.len(), move |db| {
                    db.prepare_cached("UPDATE transfers SET ranges = ?2 WHERE id = ?1")?.execute(params![id, ranges])?;
                    Ok(())
                })?;
                cut += 1;
            }
        }
        Ok(cut)
    }
}

impl Transfer {
    /// The chunk numbers still to be received.
    pub fn missing(&self) -> Vec<u64> {
        let pending: std::collections::HashSet<u64> = self.written.iter().copied().collect();
        (0..self.durable.len() as u64).filter(|index| !self.durable[*index as usize] && !pending.contains(index)).collect()
    }

    /// Writes chunk `index` once it hashes to its digest; it counts as
    /// received once synced and recorded, every few chunks or moments.
    pub fn chunk(&mut self, index: u64, bytes: &[u8]) -> Result<()> {
        let chunks = self.durable.len() as u64;
        if index >= chunks {
            return Err(Error::Invalid("no such chunk".into()));
        }
        let expected = if index + 1 == chunks { self.object.size - index * CHUNK_BYTES } else { CHUNK_BYTES };
        if bytes.len() as u64 != expected || <[u8; 32]>::from(Sha256::digest(bytes)) != self.digests[index as usize] {
            return Err(Error::Invalid(format!("chunk {index} does not match the object's manifest")));
        }
        if self.durable[index as usize] || self.written.contains(&index) {
            return Ok(());
        }
        self.file.seek(SeekFrom::Start(index * CHUNK_BYTES)).map_err(io)?;
        self.file.write_all(bytes).map_err(io)?;
        self.written.push(index);
        if self.written.len() >= SYNC_CHUNKS || self.since.elapsed() >= SYNC_AFTER {
            self.settle()?;
        }
        Ok(())
    }

    /// Syncs what was written, then records it.
    pub fn settle(&mut self) -> Result<()> {
        if self.written.is_empty() {
            return Ok(());
        }
        self.file.sync_data().map_err(io)?;
        for index in self.written.drain(..) {
            self.durable[index as usize] = true;
        }
        self.since = Instant::now();
        let (id, ranges) = (self.id, encode(&self.durable));
        self.library.database().write_relaxed_now_at(Priority::Normal, 64 + ranges.len(), move |db| {
            db.prepare_cached("UPDATE transfers SET ranges = ?2 WHERE id = ?1")?.execute(params![id, ranges])?;
            Ok(())
        })?;
        Ok(())
    }

    /// Once every chunk is in, checks the whole object, places it as a new
    /// life and leases it to `holder`.
    pub fn finish(mut self, holder: &str, ttl: Duration) -> Result<Lease> {
        self.settle()?;
        if self.durable.iter().any(|flag| !flag) {
            return Err(Error::Invalid("the transfer is missing chunks".into()));
        }
        self.file.seek(SeekFrom::Start(0)).map_err(io)?;
        let mut hasher = Sha256::new();
        let mut window = vec![0u8; 1024 * 1024];
        let mut read_total = 0u64;
        loop {
            let read = self.file.read(&mut window).map_err(io)?;
            if read == 0 {
                break;
            }
            hasher.update(&window[..read]);
            read_total += read as u64;
        }
        if read_total != self.object.size || <[u8; 32]>::from(hasher.finalize()) != self.object.hash {
            // Its chunks matched a manifest that was wrong: the transfer
            // goes, and the next one starts afresh from a source.
            let (library, id) = (self.library.clone(), self.id);
            drop(self.file);
            let _ = std::fs::remove_file(&self.part);
            library.database().write_now_at(Priority::Normal, 64, move |db| {
                db.prepare_cached("DELETE FROM transfers WHERE id = ?1")?.execute([id])?;
                Ok(())
            })?;
            return Err(Error::Invalid("the received object does not hash to what it was asked as".into()));
        }
        self.file.sync_all().map_err(io)?;
        let (object, id) = (self.object, self.id);
        let manifest: Vec<u8> = self.digests.iter().flatten().copied().collect();
        let life = self.library.database().write_now_at(Priority::Normal, 256, move |db| {
            if present(db, &object.hash)?.is_some() {
                return Ok(None);
            }
            let life: i64 = db
                .prepare_cached("SELECT COALESCE(MAX(life), 0) + 1 FROM objects WHERE hash = ?1")?
                .query_row([&object.hash[..]], |row| row.get(0))?;
            db.prepare_cached(
                "INSERT INTO objects (hash, life, size, state, placed, unreferenced, chunk) VALUES (?1, ?2, ?3, 0, ?4, NULL, ?5)",
            )?
            .execute(params![&object.hash[..], life, object.size as i64, now(), CHUNK_BYTES as i64])?;
            Ok(Some(life))
        })?;
        let library = self.library.clone();
        let holder = holder.to_string();
        let Some(life) = life else {
            // Someone else placed it meanwhile: this copy goes.
            drop(self.file);
            let _ = std::fs::remove_file(&self.part);
            library.database().write_now_at(Priority::Normal, 64, move |db| {
                db.prepare_cached("DELETE FROM transfers WHERE id = ?1")?.execute([id])?;
                Ok(())
            })?;
            return library.lease(object, &holder, ttl)?.ok_or_else(|| Error::Invalid("the object went meanwhile".into()));
        };
        drop(self.file);
        let place = library.object_file(&object.hash, life);
        fastctx_platform::durable_file::prepare_directory(place.parent().expect("an object's bucket")).map_err(io)?;
        std::fs::rename(&self.part, &place).map_err(io)?;
        sync_parent(&place).map_err(io)?;
        Ok(library.database().write_now_at(Priority::Normal, 256 + manifest.len(), move |db| {
            let changed = db
                .prepare_cached("UPDATE objects SET state = 1, placed = ?3 WHERE hash = ?1 AND life = ?2 AND state = 0")?
                .execute(params![&object.hash[..], life, now()])?;
            if changed != 1 {
                return Err(invalid("the object's life was settled by someone else while it was placed"));
            }
            db.prepare_cached("INSERT OR REPLACE INTO published (hash, life, at) VALUES (?1, ?2, ?3)")?
                .execute(params![&object.hash[..], life, now()])?;
            let mut rows = db.prepare_cached("INSERT OR REPLACE INTO object_chunks (hash, first, digests) VALUES (?1, ?2, ?3)")?;
            for (row, slice) in manifest.chunks(32 * super::objects::DIGESTS_PER_ROW as usize).enumerate() {
                rows.execute(params![&object.hash[..], row as i64 * super::objects::DIGESTS_PER_ROW as i64, slice])?;
            }
            db.prepare_cached("DELETE FROM transfers WHERE id = ?1")?.execute([id])?;
            let expires = now() + ttl.as_millis() as i64;
            db.prepare_cached("INSERT INTO leases (hash, holder, expires) VALUES (?1, ?2, ?3)")?
                .execute(params![&object.hash[..], holder, expires])?;
            Ok(Lease { id: db.last_insert_rowid(), object })
        })?)
    }
}
