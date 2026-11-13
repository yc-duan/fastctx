//! What a device's sync asks of its library (§2.1, §6.5, §6.6): the scopes
//! it began before any hub described them, with what it proposed for each;
//! the scopes it holds; how far each of its own streams is uploaded; and
//! where each stream of a scope stands.

use super::Library;
use super::frame::Header;
use super::scopes::held;
use crate::error::invalid;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{OptionalExtension, params};

/// The streams of a scope whose last frame in `holding` is `device`'s, with
/// where each reaches next.
pub(super) fn own(
    db: &fastctx_storage::rusqlite::Connection,
    scope: i64,
    holding: i64,
    device: &[u8; 32],
) -> fastctx_storage::Result<Vec<(i64, String, i64)>> {
    let streams: Vec<(i64, String, i64)> = {
        let mut statement = db.prepare_cached("SELECT id, name, next FROM streams WHERE scope = ?1 ORDER BY name")?;
        let rows = statement.query_map([scope], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let mut own = Vec::new();
    for (stream, name, next) in streams {
        if orderer(db, stream, holding)?.is_some_and(|orderer| orderer == *device) {
            own.push((stream, name, next));
        }
    }
    Ok(own)
}

/// The device that orders `stream` as this library knows: the one the
/// latest epoch known of it names, frames of it here or not (a successor
/// known takes it over at once); else the one whose frame is its last; none
/// for a stream with neither.
pub(super) fn orderer(db: &fastctx_storage::rusqlite::Connection, stream: i64, holding: i64) -> fastctx_storage::Result<Option<[u8; 32]>> {
    if let Some(device) = described_orderer(db, stream)? {
        return Ok(Some(device));
    }
    let last: Option<Vec<u8>> = db
        .prepare_cached("SELECT header FROM frames WHERE stream = ?1 AND holding = ?2 ORDER BY first DESC LIMIT 1")?
        .query_row(params![stream, holding], |row| row.get(0))
        .optional()?;
    last.map(|last| Header::decode(&last).map(|header| header.device).map_err(invalid)).transpose()
}

/// The device the latest epoch descriptor known of `stream` names, opened
/// here or not: the hub's word on who orders it.
pub(super) fn described_orderer(db: &fastctx_storage::rusqlite::Connection, stream: i64) -> fastctx_storage::Result<Option<[u8; 32]>> {
    let described: Option<Vec<u8>> = db
        .prepare_cached(
            "SELECT descriptor FROM (SELECT epoch, descriptor FROM epochs WHERE stream = ?1
             UNION ALL SELECT epoch, fact AS descriptor FROM known_epochs WHERE stream = ?1) ORDER BY epoch DESC LIMIT 1",
        )?
        .query_row(params![stream], |row| row.get(0))
        .optional()?;
    Ok(described.and_then(|fact| super::facts::EpochFact::decode(&fact).ok()).map(|fact| fact.device))
}

impl Library {
    /// A scope this device begins before any hub described it (§2.1, R26),
    /// with what it proposes the hub describe it as: readable here, never
    /// uploaded or supplied until a hub's descriptor installs it. A scope
    /// already held keeps what it has.
    pub fn propose(&self, key: &str, proposal: Vec<u8>) -> Result<i64> {
        let id = self.provisional(key)?;
        let key = key.to_string();
        self.database().write_now_at(Priority::Normal, 128 + proposal.len(), move |db| {
            db.prepare_cached("UPDATE scopes SET description = ?2 WHERE key = ?1 AND state = 'provisional'")?
                .execute(params![key, proposal])?;
            Ok(())
        })?;
        Ok(id)
    }

    /// The scopes this device holds before any hub described them, each
    /// with what it proposed.
    pub fn provisionals(&self) -> Result<Vec<(String, Option<Vec<u8>>)>> {
        Ok(self.database().read_now(|db| {
            let mut statement = db.prepare_cached("SELECT key, description FROM scopes WHERE state = 'provisional' ORDER BY key")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Every scope a hub described here, held now or not, with its
    /// descriptor.
    pub fn described_scopes(&self) -> Result<Vec<(String, Vec<u8>)>> {
        Ok(self.database().read_now(|db| {
            let mut statement =
                db.prepare_cached("SELECT key, description FROM scopes WHERE state != 'provisional' AND description IS NOT NULL ORDER BY key")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Up to `limit` objects this machine wants and does not hold that a
    /// record or frame of one of `scopes` refers to in its current holding
    /// period, first the most wanted, those in `skipped` passed over before
    /// any are counted. Which scope a wanted object was first met in does
    /// not matter: every scope that refers to it now lists it.
    pub fn wanted_of(&self, scopes: &[String], limit: usize, skipped: &[[u8; 32]]) -> Result<Vec<(String, crate::record::ObjectRef)>> {
        let scopes = scopes.to_vec();
        let skipped: std::collections::HashSet<[u8; 32]> = skipped.iter().copied().collect();
        let enough = limit + skipped.len();
        Ok(self.database().read_now(move |db| {
            // From each scope's own wanted rows (`wanted_in`), each still
            // referred to in its current period: the work follows what the
            // scopes want, not every object wanted here.
            let mut statement = db.prepare_cached(
                "SELECT w.hash, w.size, w.priority FROM scopes s
                 JOIN wanted_in i ON i.scope = s.id
                 JOIN wanted w ON w.hash = i.hash
                 WHERE s.key = ?1 AND s.holding IS NOT NULL
                   AND (EXISTS (SELECT 1 FROM object_refs r
                                WHERE r.hash = i.hash AND r.scope = s.id AND r.holding = s.holding AND r.count > 0)
                        OR EXISTS (SELECT 1 FROM frame_objects f JOIN streams t ON t.id = f.stream
                                   WHERE f.hash = i.hash AND t.scope = s.id AND f.holding = s.holding))
                   AND NOT EXISTS (SELECT 1 FROM objects o WHERE o.hash = i.hash AND +o.state = 1)
                 ORDER BY w.priority DESC, w.hash LIMIT ?2",
            )?;
            let mut candidates: Vec<(i64, [u8; 32], u64, String)> = Vec::new();
            for scope in scopes {
                let rows = statement.query_map(params![scope, enough as i64], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
                })?;
                for row in rows {
                    let (hash, size, priority) = row?;
                    let hash: [u8; 32] = hash.try_into().map_err(|_| invalid("a malformed hash"))?;
                    candidates.push((priority, hash, size as u64, scope.clone()));
                }
            }
            candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let mut seen = std::collections::HashSet::new();
            Ok(candidates
                .into_iter()
                .filter(|candidate| !skipped.contains(&candidate.1) && seen.insert(candidate.1))
                .take(limit)
                .map(|(_, hash, size, scope)| (scope, crate::record::ObjectRef { hash, size }))
                .collect())
        })?)
    }

    /// Up to `limit` objects this machine wants and does not hold that a
    /// record or frame of a scope of `hub`'s network refers to in its
    /// current holding period, as `wanted_of` gives them for those scopes,
    /// in one look however many scopes there are.
    pub fn wanted_of_hub(&self, hub: &str, limit: usize, skipped: &[[u8; 32]]) -> Result<Vec<(String, crate::record::ObjectRef)>> {
        let hub = hub.to_string();
        let skipped: std::collections::HashSet<[u8; 32]> = skipped.iter().copied().collect();
        let enough = limit + skipped.len();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT w.hash, w.size, MIN(s.key) FROM scopes s
                 JOIN wanted_in i ON i.scope = s.id
                 JOIN wanted w ON w.hash = i.hash
                 WHERE s.hub = ?1 AND s.holding IS NOT NULL
                   AND (EXISTS (SELECT 1 FROM object_refs r
                                WHERE r.hash = i.hash AND r.scope = s.id AND r.holding = s.holding AND r.count > 0)
                        OR EXISTS (SELECT 1 FROM frame_objects f JOIN streams t ON t.id = f.stream
                                   WHERE f.hash = i.hash AND t.scope = s.id AND f.holding = s.holding))
                   AND NOT EXISTS (SELECT 1 FROM objects o WHERE o.hash = i.hash AND +o.state = 1)
                 GROUP BY w.hash ORDER BY MAX(w.priority) DESC, w.hash LIMIT ?2",
            )?;
            let rows = statement.query_map(params![hub, enough as i64], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
            })?;
            let mut wanted = Vec::new();
            for row in rows {
                let (hash, size, scope) = row?;
                let hash: [u8; 32] = hash.try_into().map_err(|_| invalid("a malformed hash"))?;
                if skipped.contains(&hash) {
                    continue;
                }
                wanted.push((scope, crate::record::ObjectRef { hash, size: size as u64 }));
                if wanted.len() == limit {
                    break;
                }
            }
            Ok(wanted)
        })?)
    }

    /// The scopes a hub of `hub`'s network described here, held now or not.
    pub fn described_of(&self, hub: &str) -> Result<Vec<String>> {
        let hub = hub.to_string();
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached("SELECT key FROM scopes WHERE hub = ?1 AND described = 1 ORDER BY key")?;
            let rows = statement.query_map([&hub], |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// The scopes this device holds under a hub's descriptor, each with its
    /// descriptor and current holding period.
    pub fn live_scopes(&self) -> Result<Vec<(String, Vec<u8>, i64)>> {
        Ok(self.database().read_now(|db| {
            let mut statement = db.prepare_cached(
                "SELECT key, description, holding FROM scopes WHERE state = 'live' AND holding IS NOT NULL ORDER BY key",
            )?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get::<_, Option<Vec<u8>>>(1)?.unwrap_or_default(), row.get(2)?)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Whether this device holds `scope` under a hub's descriptor: one look
    /// at that scope alone (AH03).
    pub fn is_live(&self, scope: &str) -> Result<bool> {
        let scope = scope.to_string();
        Ok(self.database().read_now(move |db| {
            Ok(db
                .prepare_cached("SELECT 1 FROM scopes WHERE key = ?1 AND state = 'live' AND holding IS NOT NULL")?
                .query_row([&scope], |_| Ok(()))
                .optional()?
                .is_some())
        })?)
    }

    /// Where each stream of `scope` this device does not order stands here,
    /// as `standing` gives it, read with which it orders in one look (§7.5).
    pub fn replica_standing(&self, scope: &str, device: &[u8; 32]) -> Result<Vec<(String, u64, [u8; 32])>> {
        let (scope, device) = (scope.to_string(), *device);
        Ok(self.database().read_now(move |db| {
            // One snapshot for which streams are this device's and where each
            // stands: nothing committed between the two reads.
            let snapshot = db.unchecked_transaction()?;
            let db: &fastctx_storage::rusqlite::Connection = &snapshot;
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            let own: std::collections::BTreeSet<String> = own(db, scope_id, holding, &device)?.into_iter().map(|(_, name, _)| name).collect();
            let mut statement = db.prepare_cached(
                "SELECT s.name, c.sequence, c.head FROM cursors c JOIN streams s ON s.id = c.stream
                 WHERE s.scope = ?1 AND c.holding = ?2 ORDER BY s.name",
            )?;
            let rows = statement.query_map(params![scope_id, holding], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<Vec<u8>>>(2)?))
            })?;
            let mut standing = Vec::new();
            for row in rows {
                let (name, sequence, head) = row?;
                if own.contains(&name) {
                    continue;
                }
                let head = head.unwrap_or_default().try_into().map_err(|_| invalid("a malformed cursor"))?;
                standing.push((name, sequence as u64, head));
            }
            Ok(standing)
        })?)
    }

    /// The streams of `scope` this device orders now — its current epoch is
    /// `device`'s, or, where no descriptor of it is here, its last frame:
    /// its own, where the hub follows it, never the other way (§7.5).
    pub fn own_streams(&self, scope: &str, device: &[u8; 32]) -> Result<Vec<String>> {
        let (scope, device) = (scope.to_string(), *device);
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            Ok(own(db, scope_id, holding, &device)?.into_iter().map(|(_, name, _)| name).collect())
        })?)
    }

    /// The device that orders `stream` of `scope` as this library knows
    /// (`orderer`); none for a stream not begun here.
    pub fn orderer(&self, scope: &str, stream: &str) -> Result<Option<[u8; 32]>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(None) };
            let id: Option<i64> = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| row.get(0))
                .optional()?;
            match id {
                Some(id) => orderer(db, id, holding),
                None => Ok(None),
            }
        })?)
    }

    /// The streams of `scope` this device orders now, with the sequence
    /// uploading goes on from, for those not all uploaded.
    pub fn uploads(&self, scope: &str, device: &[u8; 32]) -> Result<Vec<(String, u64)>> {
        let (scope, device) = (scope.to_string(), *device);
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, state, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            if state != "live" {
                return Ok(Vec::new());
            }
            let mut uploads = Vec::new();
            for (stream, name, next) in own(db, scope_id, holding, &device)? {
                let uploaded: i64 = db
                    .prepare_cached("SELECT uploaded FROM outbox WHERE stream = ?1")?
                    .query_row([stream], |row| row.get(0))
                    .optional()?
                    .unwrap_or(0);
                if uploaded < next {
                    uploads.push((name, uploaded as u64));
                }
            }
            Ok(uploads)
        })?)
    }

    /// Each stream of `scope` here, with the sequence its applied prefix
    /// in the current period reaches next.
    pub fn streams(&self, scope: &str) -> Result<Vec<(String, u64)>> {
        let scope = scope.to_string();
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            let mut statement = db.prepare_cached(
                "SELECT s.name, COALESCE((SELECT sequence + 1 FROM cursors c WHERE c.stream = s.id AND c.holding = ?2), 0)
                 FROM streams s WHERE s.scope = ?1 ORDER BY s.name",
            )?;
            let rows = statement.query_map(params![scope_id, holding], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// Up to `limit` objects this machine wants and does not hold, each with
    /// the scope a record that refers to it came in, first the most wanted.
    pub fn wanted(&self, limit: usize) -> Result<Vec<(Option<String>, crate::record::ObjectRef)>> {
        Ok(self.database().read_now(move |db| {
            let mut statement = db.prepare_cached(
                "SELECT w.hash, w.size, w.hint FROM wanted w
                 WHERE NOT EXISTS (SELECT 1 FROM objects o WHERE o.hash = w.hash AND +o.state = 1)
                 ORDER BY w.priority DESC, w.hash LIMIT ?1",
            )?;
            let rows = statement.query_map([limit as i64], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<String>>(2)?))
            })?;
            let mut wanted = Vec::new();
            for row in rows {
                let (hash, size, scope) = row?;
                let hash: [u8; 32] = hash.try_into().map_err(|_| invalid("a malformed hash"))?;
                wanted.push((scope, crate::record::ObjectRef { hash, size: size as u64 }));
            }
            Ok(wanted)
        })?)
    }

    /// `hash` is wanted no more: it is here, or nothing gives it.
    pub fn unwant(&self, hash: &[u8; 32]) -> Result<()> {
        let hash = *hash;
        Ok(self.database().write_relaxed_now_at(Priority::Background, 64, move |db| {
            db.prepare_cached("DELETE FROM wanted WHERE hash = ?1")?.execute([&hash[..]])?;
            db.prepare_cached("DELETE FROM wanted_in WHERE hash = ?1")?.execute([&hash[..]])?;
            Ok(())
        })?)
    }
}
