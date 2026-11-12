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
fn own(
    db: &fastctx_storage::rusqlite::Connection,
    scope: i64,
    holding: i64,
    device: &[u8; 32],
) -> fastctx_storage::Result<Vec<(i64, String, i64)>> {
    let streams: Vec<(i64, String, i64, i64)> = {
        let mut statement = db.prepare_cached("SELECT id, name, next, epoch FROM streams WHERE scope = ?1 ORDER BY name")?;
        let rows = statement.query_map([scope], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let mut own = Vec::new();
    for (stream, name, next, epoch) in streams {
        // The device the stream's current epoch names orders it, frames of
        // it here or not.
        let described: Option<Vec<u8>> = db
            .prepare_cached(
                "SELECT descriptor FROM epochs WHERE stream = ?1 AND epoch = ?2
                 UNION ALL SELECT fact FROM known_epochs WHERE stream = ?1 AND epoch = ?2 LIMIT 1",
            )?
            .query_row(params![stream, epoch], |row| row.get(0))
            .optional()?;
        if let Some(fact) = described.and_then(|fact| super::facts::EpochFact::decode(&fact).ok()) {
            if fact.device == *device {
                own.push((stream, name, next));
            }
            continue;
        }
        let last: Option<Vec<u8>> = db
            .prepare_cached("SELECT header FROM frames WHERE stream = ?1 AND holding = ?2 ORDER BY first DESC LIMIT 1")?
            .query_row(params![stream, holding], |row| row.get(0))
            .optional()?;
        let Some(last) = last else { continue };
        if Header::decode(&last).map_err(invalid)?.device == *device {
            own.push((stream, name, next));
        }
    }
    Ok(own)
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

    /// Where each stream of `scope` this device does not order stands here,
    /// as `standing` gives it, read with which it orders in one look (§7.5).
    pub fn replica_standing(&self, scope: &str, device: &[u8; 32]) -> Result<Vec<(String, u64, [u8; 32])>> {
        let (scope, device) = (scope.to_string(), *device);
        Ok(self.database().read_now(move |db| {
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
                 WHERE NOT EXISTS (SELECT 1 FROM objects o WHERE o.hash = w.hash AND o.state = 1)
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
            Ok(())
        })?)
    }
}
