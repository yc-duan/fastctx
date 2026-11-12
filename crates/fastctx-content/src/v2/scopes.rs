//! Scopes and their holding periods on this machine (§2.1, §4.5).
//!
//! Each time a scope is installed — first granted, granted again after a
//! revocation, held under another eligibility — it opens a new holding
//! period, and every row written for it carries that number. A revocation
//! ends the period at once in the same transaction that disables the scope
//! and records its purge; the purge then removes that period's rows in
//! batches, and while a newer period exists it keeps each of its index rows
//! as a revision floor instead (§2.4).

use super::Library;
use super::objects::now;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

/// Rows one purge pass removes.
const PURGE_PASS: i64 = 2048;

/// What one purge pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Purged {
    pub rows: usize,
    pub floors: usize,
    pub finished: usize,
}

/// A scope's id and its current holding period, when it is held.
pub(super) fn held(db: &Connection, key: &str) -> fastctx_storage::Result<Option<(i64, String, Option<i64>)>> {
    Ok(db
        .prepare_cached("SELECT id, state, holding FROM scopes WHERE key = ?1")?
        .query_row([key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?)
}

fn open_holding(db: &Connection, scope: i64, acl_revision: i64) -> fastctx_storage::Result<i64> {
    let holding: i64 = db
        .prepare_cached("SELECT COALESCE(MAX(holding), 0) + 1 FROM holdings WHERE scope = ?1")?
        .query_row([scope], |row| row.get(0))?;
    db.prepare_cached("INSERT INTO holdings (scope, holding, started, ended, acl_revision) VALUES (?1, ?2, ?3, NULL, ?4)")?
        .execute(params![scope, holding, now(), acl_revision])?;
    Ok(holding)
}

impl Library {
    /// The current holding period of `scope` here, if it is held: what a
    /// write made now carries (`Commit::holding`).
    pub fn holding(&self, scope: &str) -> Result<Option<i64>> {
        let scope = scope.to_string();
        Ok(self.database().read_now(move |db| Ok(held(db, &scope)?.and_then(|(_, _, holding)| holding)))?)
    }

    /// A scope this machine has before any hub described it (§2.1, R26):
    /// readable here, never uploaded or supplied until it is installed.
    pub fn provisional(&self, key: &str) -> Result<i64> {
        let key = key.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 128, move |db| {
            if let Some((id, _, _)) = held(db, &key)? {
                return Ok(id);
            }
            db.prepare_cached("INSERT INTO scopes (key, description, state, holding) VALUES (?1, NULL, 'provisional', NULL)")?
                .execute([&key])?;
            let id = db.last_insert_rowid();
            let holding = open_holding(db, id, 0)?;
            db.prepare_cached("UPDATE scopes SET holding = ?2 WHERE id = ?1")?.execute(params![id, holding])?;
            Ok(id)
        })?)
    }

    /// Installs a scope under its hub-signed description: a provisional one
    /// keeps its rows and period; one not held opens a new period beside
    /// whatever of an ended period is still being purged.
    pub fn install(&self, key: &str, description: Vec<u8>, acl_revision: i64) -> Result<i64> {
        let key = key.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 256 + description.len(), move |db| {
            match held(db, &key)? {
                Some((id, state, Some(holding))) if state == "provisional" || state == "live" => {
                    db.prepare_cached("UPDATE scopes SET state = 'live', description = ?2 WHERE id = ?1")?
                        .execute(params![id, description])?;
                    Ok(holding)
                }
                Some((id, _, _)) => {
                    let holding = open_holding(db, id, acl_revision)?;
                    db.prepare_cached("UPDATE scopes SET state = 'live', description = ?2, holding = ?3 WHERE id = ?1")?
                        .execute(params![id, description, holding])?;
                    Ok(holding)
                }
                None => {
                    db.prepare_cached("INSERT INTO scopes (key, description, state, holding) VALUES (?1, ?2, 'live', NULL)")?
                        .execute(params![key, description])?;
                    let id = db.last_insert_rowid();
                    let holding = open_holding(db, id, acl_revision)?;
                    db.prepare_cached("UPDATE scopes SET holding = ?2 WHERE id = ?1")?.execute(params![id, holding])?;
                    Ok(holding)
                }
            }
        })?)
    }

    /// Revokes a scope here: disabled, its period ended and its purge
    /// recorded in one transaction; readers see none of it from then on.
    pub fn revoke(&self, key: &str) -> Result<()> {
        let key = key.to_string();
        Ok(self.database().write_now_at(Priority::Control, 128, move |db| {
            let Some((id, _, Some(holding))) = held(db, &key)? else { return Ok(()) };
            db.prepare_cached("UPDATE scopes SET state = 'disabled', holding = NULL WHERE id = ?1")?.execute([id])?;
            db.prepare_cached("UPDATE holdings SET ended = ?3 WHERE scope = ?1 AND holding = ?2")?
                .execute(params![id, holding, now()])?;
            db.prepare_cached("INSERT OR IGNORE INTO purges (scope, holding, phase, position) VALUES (?1, ?2, 'rows', NULL)")?
                .execute(params![id, holding])?;
            Ok(())
        })?)
    }

    /// One pass of every purge: an ended period's frames, references and
    /// cursors go; its index rows go too, or become revision floors while
    /// the scope has a newer period.
    pub fn purge(&self) -> Result<Purged> {
        Ok(self.database().write_now_at(Priority::Background, 4096, move |db| {
            let purges: Vec<(i64, i64)> = {
                let mut statement = db.prepare_cached("SELECT scope, holding FROM purges")?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            let mut purged = Purged::default();
            for (scope, holding) in purges {
                let current: Option<i64> =
                    db.prepare_cached("SELECT holding FROM scopes WHERE id = ?1")?.query_row([scope], |row| row.get(0))?;
                purged.rows += super::rejected::end_resubmits(db, scope, holding)?;
                let mut done = 0usize;
                // What only a hub's parked or refused frames held waits a
                // grace from here (I2).
                for table in ["parked_objects", "refused_objects"] {
                    db.prepare_cached(&format!(
                        "UPDATE objects SET unreferenced = ?4 WHERE state = 1 AND hash IN (SELECT hash FROM {table}
                         WHERE stream IN (SELECT id FROM streams WHERE scope = ?1) AND holding = ?2
                         ORDER BY stream, epoch, first, holding, hash LIMIT ?3)"
                    ))?
                    .execute(params![scope, holding, PURGE_PASS, now()])?;
                }
                // So does what only evidence of the period held; its holds go
                // before it does.
                let evidence = "SELECT holder FROM evidence WHERE stream IN (SELECT id FROM streams WHERE scope = ?1) AND holding = ?2
                                ORDER BY stream, epoch, first, digest LIMIT ?3";
                db.prepare_cached(&format!(
                    "UPDATE objects SET unreferenced = ?4 WHERE state = 1 AND hash IN
                     (SELECT hash FROM object_holds WHERE kind = 'evidence' AND holder IN ({evidence}))"
                ))?
                .execute(params![scope, holding, PURGE_PASS, now()])?;
                done += db
                    .prepare_cached(&format!("DELETE FROM object_holds WHERE kind = 'evidence' AND holder IN ({evidence})"))?
                    .execute(params![scope, holding, PURGE_PASS])?;
                for (table, key) in [
                    ("frame_objects", "stream, epoch, first, holding, hash"),
                    ("frames", "stream, epoch, first, holding"),
                    ("cursors", "stream, holding"),
                    ("parked_objects", "stream, epoch, first, holding, hash"),
                    ("refused_objects", "stream, epoch, first, holding, hash"),
                    ("parked", "stream, epoch, first, holding"),
                    ("evidence", "stream, epoch, first, digest"),
                ] {
                    done += db
                        .prepare_cached(&format!(
                            "DELETE FROM {table} WHERE ({key}) IN (SELECT {key} FROM {table}
                             WHERE stream IN (SELECT id FROM streams WHERE scope = ?1) AND holding = ?2 ORDER BY {key} LIMIT ?3)"
                        ))?
                        .execute(params![scope, holding, PURGE_PASS])?;
                }
                // Index rows: floors where a newer period fills in, else gone.
                // Their references go with the period's below.
                let floors = if current.is_some() {
                    db.prepare_cached(
                        "UPDATE items SET state = 2, body = NULL, objects = NULL WHERE (stream, place, item) IN
                         (SELECT stream, place, item FROM items
                          WHERE stream IN (SELECT id FROM streams WHERE scope = ?1) AND holding = ?2 AND state != 2 LIMIT ?3)",
                    )?
                    .execute(params![scope, holding, PURGE_PASS])?
                } else {
                    db.prepare_cached(
                        "DELETE FROM items WHERE (stream, place, item) IN
                         (SELECT stream, place, item FROM items
                          WHERE stream IN (SELECT id FROM streams WHERE scope = ?1) AND holding = ?2 LIMIT ?3)",
                    )?
                    .execute(params![scope, holding, PURGE_PASS])?
                };
                if current.is_some() {
                    purged.floors += floors;
                } else {
                    // Held in no period, the scope's floors of any earlier
                    // period guard nothing: a later grant starts afresh.
                    done += db
                        .prepare_cached("DELETE FROM items WHERE stream IN (SELECT id FROM streams WHERE scope = ?1) AND state = 2")?
                        .execute([scope])?;
                }
                done += floors;
                // Collection waits a grace after the last reference went (I2).
                db.prepare_cached(
                    "UPDATE objects SET unreferenced = ?3 WHERE state = 1 AND hash IN
                     (SELECT hash FROM object_refs WHERE scope = ?1 AND holding = ?2)",
                )?
                .execute(params![scope, holding, now()])?;
                done += db
                    .prepare_cached("DELETE FROM object_refs WHERE scope = ?1 AND holding = ?2")?
                    .execute(params![scope, holding])?;
                purged.rows += done;
                if done == 0 {
                    db.prepare_cached("DELETE FROM purges WHERE scope = ?1 AND holding = ?2")?.execute(params![scope, holding])?;
                    purged.finished += 1;
                }
            }
            Ok(purged)
        })?)
    }
}
