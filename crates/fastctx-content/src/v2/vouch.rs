//! On a hub, letting go of its own copy of a large object the machine a
//! conversation lives on keeps (`fastctx_types::assets::VOUCH`): the device
//! ordering each stream of a conversation vouches for the objects it holds
//! whole that the stream's frames refer to, and an object every reference of
//! which is a conversation's stream a machine orders, vouched for lately by
//! the device ordering it now in its epoch now — a device that may hold the
//! scope still — and held by nothing else, may go here. It goes as any
//! collected object goes (`collect`): a tombstone, its file, its row; the
//! frames referring to it stay, and what asks for it is told it is not
//! here. Small objects, kept in the bytes database, are left.

use super::Library;
use super::handover::epoch_of;
use super::objects::{TOMBSTONE, now};
use super::scopes::held;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};

/// How long what a device vouched for counts: past it, it is vouched for
/// again or kept.
pub const VOUCHED_FOR: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// A scope and the key of a device vouching for objects of it, as a hub
/// judges whether that device may hold the scope now (`Library::vouchers`).
pub type Voucher = (String, [u8; 32]);

/// What going through the large objects here came to: those let go, or
/// that may be, those kept, by why, and of those let go, the bytes given
/// back and those whose bytes could not be deleted yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reclaimed {
    pub objects: u64,
    pub bytes: u64,
    /// Why each kept was, with how many and their bytes.
    pub kept: BTreeMap<&'static str, (u64, u64)>,
    /// Of those let go, the bytes deleted.
    pub freed: u64,
    /// Of those let go, how many are tombstones still, their bytes not
    /// deleted yet (`Library::settle_tombstones`).
    pub stuck: u64,
}

/// Whether a scope's key is a conversation's: a machine's, or a body's but
/// for the body's log and records.
fn conversation(key: &str) -> bool {
    key.starts_with("c/") || (key.starts_with("b/") && !key.ends_with("/log") && !key.ends_with("/records"))
}

/// The streams that refer to `hash`: by its frames, or by index rows that
/// count it.
fn referring(db: &Connection, hash: &[u8]) -> fastctx_storage::Result<BTreeSet<i64>> {
    let mut streams = BTreeSet::new();
    let mut statement = db.prepare_cached(
        "SELECT DISTINCT stream FROM frame_objects WHERE hash = ?1
         UNION SELECT DISTINCT stream FROM object_refs WHERE hash = ?1 AND count > 0",
    )?;
    let rows = statement.query_map([hash], |row| row.get::<_, i64>(0))?;
    for row in rows {
        streams.insert(row?);
    }
    Ok(streams)
}

/// Who vouched for `hash` as the device ordering `stream` now, in its epoch
/// now, and the stream's scope: none where none did, or not as it orders now.
fn voucher(db: &Connection, stream: i64, hash: &[u8]) -> fastctx_storage::Result<std::result::Result<(Voucher, i64), &'static str>> {
    let found: Option<(String, String)> = db
        .prepare_cached("SELECT c.key, s.name FROM streams s JOIN scopes c ON c.id = s.scope WHERE s.id = ?1")?
        .query_row([stream], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((key, name)) = found else { return Ok(Err("a stream that refers to it is not known")) };
    if !conversation(&key) {
        return Ok(Err("something not a conversation's refers to it"));
    }
    if super::hub_ordered::hub_orders(&name) {
        return Ok(Err("a stream the hub orders refers to it"));
    }
    let last: Option<i64> =
        db.prepare_cached("SELECT MAX(epoch) FROM hub_epochs WHERE stream = ?1")?.query_row([stream], |row| row.get(0))?;
    let Some(epoch) = last.and_then(|last| u64::try_from(last).ok()).map(|last| epoch_of(db, stream, last)).transpose()?.flatten()
    else {
        return Ok(Err("no epoch of a stream that refers to it is signed"));
    };
    let vouched: Option<(i64, Vec<u8>, i64)> = db
        .prepare_cached("SELECT epoch, key, at FROM vouched WHERE stream = ?1 AND hash = ?2")?
        .query_row(params![stream, hash], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    match vouched {
        Some((at, key_said, when)) if at as u64 == epoch.epoch && key_said[..] == epoch.key[..] => Ok(Ok(((key, epoch.key), when))),
        Some(_) => Ok(Err("vouched for in an epoch that is not the stream's now")),
        None => Ok(Err("not vouched for by the device ordering a stream that refers to it")),
    }
}

/// Why the object `hash` may not go here, if it may not: something but the
/// frames and index rows of conversations' streams machines order holds it,
/// one of those streams' devices ordering it now did not vouch for it in its
/// epoch now, did so too long ago, or may not hold the scope now (`able`).
fn kept_why(db: &Connection, hash: &[u8], able: &BTreeSet<Voucher>) -> fastctx_storage::Result<Option<&'static str>> {
    for (table, why) in [
        ("parked_objects", "a frame waiting for it refers to it"),
        ("refused_objects", "a refused frame refers to it"),
        ("object_holds", "it is held for a write or as evidence"),
    ] {
        let held: bool = db
            .prepare_cached(&format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE hash = ?1)"))?
            .query_row([hash], |row| row.get(0))?;
        if held {
            return Ok(Some(why));
        }
    }
    let leased: bool = db
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM leases WHERE hash = ?1 AND expires >= ?2)")?
        .query_row(params![hash, now()], |row| row.get(0))?;
    if leased {
        return Ok(Some("it is being sent or fetched"));
    }
    let streams = referring(db, hash)?;
    if streams.is_empty() {
        return Ok(Some("nothing refers to it: it is collected as anything is"));
    }
    let lately = now() - VOUCHED_FOR.as_millis() as i64;
    for stream in streams {
        let (voucher, when) = match voucher(db, stream, hash)? {
            Ok(found) => found,
            Err(why) => return Ok(Some(why)),
        };
        if when < lately {
            return Ok(Some("vouched for too long ago"));
        }
        if !able.contains(&voucher) {
            return Ok(Some("the device that vouched for it may not hold its conversation now"));
        }
    }
    Ok(None)
}

/// A stream this device orders and the large objects here its frames refer
/// to, to be vouched for (`Library::vouchable`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vouchable {
    pub scope: String,
    pub stream: String,
    pub epoch: u64,
    pub hashes: Vec<[u8; 32]>,
}

impl Library {
    /// On a machine, up to `limit` of the conversation streams of the scopes
    /// of the hub `hub` that `device` orders, a machine's to order, after
    /// `after` (scope and stream, in order), each with its epoch now and the
    /// large objects present here its frames in the holding period now refer
    /// to — none where there are none.
    pub fn vouchable(&self, hub: &str, device: &[u8; 32], after: Option<(String, String)>, limit: usize) -> Result<Vec<Vouchable>> {
        let (hub, device) = (hub.to_string(), *device);
        let row_bytes = super::ROW_BYTES as i64;
        Ok(self.database().read_now(move |db| {
            let scopes: Vec<(i64, String, i64)> = {
                let mut statement = db.prepare_cached(
                    "SELECT id, key, holding FROM scopes WHERE state = 'live' AND holding IS NOT NULL AND hub = ?1 ORDER BY key",
                )?;
                let rows = statement.query_map([&hub], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            let mut found = Vec::new();
            for (scope_id, key, holding) in scopes {
                if !conversation(&key) || after.as_ref().is_some_and(|(scope, _)| key < *scope) {
                    continue;
                }
                for (stream_id, name, _) in super::device::own(db, scope_id, holding, &device)? {
                    if super::hub_ordered::hub_orders(&name)
                        || after.as_ref().is_some_and(|(scope, stream)| key == *scope && name <= *stream)
                    {
                        continue;
                    }
                    let epoch: i64 =
                        db.prepare_cached("SELECT epoch FROM streams WHERE id = ?1")?.query_row([stream_id], |row| row.get(0))?;
                    let hashes: Vec<[u8; 32]> = {
                        let mut statement = db.prepare_cached(
                            "SELECT DISTINCT f.hash FROM frame_objects f
                             WHERE f.stream = ?1 AND f.holding = ?2
                               AND EXISTS (SELECT 1 FROM objects o WHERE o.hash = f.hash AND o.state = 1 AND o.size > ?3)
                             ORDER BY f.hash",
                        )?;
                        let rows = statement.query_map(params![stream_id, holding, row_bytes], |row| row.get::<_, Vec<u8>>(0))?;
                        let mut hashes = Vec::new();
                        for row in rows {
                            if let Ok(hash) = <[u8; 32]>::try_from(row?.as_slice()) {
                                hashes.push(hash);
                            }
                        }
                        hashes
                    };
                    if !hashes.is_empty() {
                        found.push(Vouchable { scope: key.clone(), stream: name, epoch: epoch.max(0) as u64, hashes });
                        if found.len() >= limit {
                            return Ok(found);
                        }
                    }
                }
            }
            Ok(found)
        })?)
    }

    /// On a hub, keeps that the device of `key`, ordering `stream` of `scope`
    /// in `epoch`, holds whole each object of `hashes` that the stream's
    /// frames here refer to; the others are passed by. Done only while that
    /// device orders it in that epoch, as the hub signed it — judged in the
    /// same transaction: how many were kept, none where it does not.
    pub fn vouch(&self, scope: &str, stream: &str, epoch: u64, key: &[u8; 32], hashes: &[[u8; 32]]) -> Result<Option<usize>> {
        let (scope, stream, key, hashes) = (scope.to_string(), stream.to_string(), *key, hashes.to_vec());
        let kept = self.database().write_now_at(Priority::Normal, 64 * hashes.len() + 64, move |db| {
            let Some((scope_id, _, Some(_))) = held(db, &scope)? else { return Ok(None) };
            let Some(stream_id) = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| row.get::<_, i64>(0))
                .optional()?
            else {
                return Ok(None);
            };
            let last: Option<i64> =
                db.prepare_cached("SELECT MAX(epoch) FROM hub_epochs WHERE stream = ?1")?.query_row([stream_id], |row| row.get(0))?;
            let now_ordering = last
                .and_then(|last| u64::try_from(last).ok())
                .map(|last| epoch_of(db, stream_id, last))
                .transpose()?
                .flatten()
                .is_some_and(|ordering| ordering.epoch == epoch && ordering.key == key);
            if !now_ordering {
                return Ok(None);
            }
            let mut kept = 0;
            let mut keep = db.prepare_cached(
                "INSERT INTO vouched (stream, hash, epoch, key, at)
                 SELECT ?1, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM frame_objects WHERE stream = ?1 AND hash = ?2)
                 ON CONFLICT (stream, hash) DO UPDATE SET epoch = excluded.epoch, key = excluded.key, at = excluded.at",
            )?;
            for hash in &hashes {
                kept += keep.execute(params![stream_id, &hash[..], epoch as i64, &key[..], now()])?;
            }
            Ok(Some(kept))
        })?;
        Ok(kept)
    }

    /// The scopes and vouching devices' keys the next `limit` large objects
    /// here after `after` are vouched for under, for the hub to judge which
    /// of those devices may hold those scopes now (`reclaim_vouched`).
    pub fn vouchers(&self, after: Option<[u8; 32]>, limit: usize) -> Result<BTreeSet<Voucher>> {
        let row_bytes = super::ROW_BYTES as i64;
        Ok(self.database().read_now(move |db| {
            let start = after.map(|hash| hash.to_vec()).unwrap_or_default();
            let large: Vec<Vec<u8>> = {
                let mut statement =
                    db.prepare_cached("SELECT hash FROM objects WHERE state = 1 AND size > ?1 AND hash > ?2 ORDER BY hash LIMIT ?3")?;
                let rows = statement.query_map(params![row_bytes, start, limit as i64], |row| row.get(0))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            let mut found = BTreeSet::new();
            for hash in large {
                for stream in referring(db, &hash)? {
                    if let Ok((voucher, _)) = voucher(db, stream, &hash)? {
                        found.insert(voucher);
                    }
                }
            }
            Ok(found)
        })?)
    }

    /// Goes through up to `limit` large objects here after `after`, by hash:
    /// counts those that may go and those kept, by why, or, `apply`, lets go
    /// those that may — judged again in the transaction that makes each a
    /// tombstone, the devices that vouched for them among `able`, their files
    /// and rows deleted after, one that cannot be deleted left a tombstone
    /// and said (`stuck`). One longer than `longest` is kept: the machines
    /// take none so long from one another. Where the next pass goes on from,
    /// none when all were gone through, and what was found.
    pub fn reclaim_vouched(
        &self,
        after: Option<[u8; 32]>,
        limit: usize,
        longest: u64,
        apply: bool,
        able: BTreeSet<Voucher>,
    ) -> Result<(Option<[u8; 32]>, Reclaimed)> {
        let row_bytes = super::ROW_BYTES as i64;
        let longest = i64::try_from(longest).unwrap_or(i64::MAX);
        let (next, mut found, doomed) = self.database().write_now_at(Priority::Background, 1024, move |db| {
            let start = after.map(|hash| hash.to_vec()).unwrap_or_default();
            let large: Vec<(Vec<u8>, i64, i64)> = {
                let mut statement = db.prepare_cached(
                    "SELECT hash, life, size FROM objects WHERE state = 1 AND size > ?1 AND hash > ?2 ORDER BY hash LIMIT ?3",
                )?;
                let rows = statement.query_map(params![row_bytes, start, limit as i64], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            let next = (large.len() == limit).then(|| large.last().map(|(hash, ..)| hash.clone())).flatten();
            let mut found = Reclaimed::default();
            let mut doomed = Vec::new();
            for (hash, life, size) in large {
                let why = if size > longest { Some("longer than the machines take from one another") } else { kept_why(db, &hash, &able)? };
                match why {
                    Some(why) => {
                        let entry = found.kept.entry(why).or_default();
                        entry.0 += 1;
                        entry.1 += size.max(0) as u64;
                    }
                    None => {
                        found.objects += 1;
                        found.bytes += size.max(0) as u64;
                        if apply {
                            db.prepare_cached("UPDATE objects SET state = ?3 WHERE hash = ?1 AND life = ?2 AND state = 1")?
                                .execute(params![&hash, life, TOMBSTONE])?;
                            doomed.push((hash, life, size.max(0) as u64));
                        }
                    }
                }
            }
            Ok((next, found, doomed))
        })?;
        for (hash, life, size) in doomed {
            let hash: [u8; 32] = hash.try_into().map_err(|_| crate::Error::Invalid("a malformed object hash".into()))?;
            match self.delete_life(&hash, life, size) {
                Ok(()) => found.freed += size,
                Err(_) => found.stuck += 1,
            }
        }
        let next = next.map(|hash| hash.try_into().map_err(|_| crate::Error::Invalid("a malformed object hash".into()))).transpose()?;
        Ok((next, found))
    }

    /// Deletes the bytes and rows of up to `limit` tombstones here, as a
    /// start does: those a deletion cut short or could not finish. How many
    /// went and their bytes, and how many could not.
    pub fn settle_tombstones(&self, limit: usize) -> Result<(u64, u64, u64)> {
        let tombstones: Vec<(Vec<u8>, i64, i64)> = self.database().read_now(move |db| {
            let mut statement = db.prepare_cached("SELECT hash, life, size FROM objects WHERE state = ?1 LIMIT ?2")?;
            let rows = statement.query_map(params![TOMBSTONE, limit as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        let (mut gone, mut bytes, mut stuck) = (0, 0, 0);
        for (hash, life, size) in tombstones {
            let Ok(hash) = <[u8; 32]>::try_from(hash.as_slice()) else { continue };
            match self.delete_life(&hash, life, size.max(0) as u64) {
                Ok(()) => {
                    gone += 1;
                    bytes += size.max(0) as u64;
                }
                Err(_) => stuck += 1,
            }
        }
        Ok((gone, bytes, stuck))
    }

    /// Whether the present object `hash` is here whole: every byte of it read
    /// back, of its length, hashing to it.
    pub fn verify_object(&self, hash: &[u8; 32]) -> Result<bool> {
        use sha2::{Digest, Sha256};
        let Some(size) = self.size(hash)? else { return Ok(false) };
        let mut hasher = Sha256::new();
        let mut offset = 0u64;
        while offset < size {
            let Ok(bytes) = self.read(hash, offset, super::READ_BYTES) else { return Ok(false) };
            if bytes.is_empty() {
                return Ok(false);
            }
            hasher.update(&bytes);
            offset += bytes.len() as u64;
        }
        Ok(offset == size && <[u8; 32]>::from(hasher.finalize()) == *hash)
    }
}
