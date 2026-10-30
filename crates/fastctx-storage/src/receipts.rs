//! Indexed receipts expire only after durable transport acknowledgement or an
//! independent terminal business effect. Unproven legacy receipts have no expiry.

use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::time::Duration;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS receipts (
 namespace TEXT NOT NULL, scope TEXT NOT NULL, id TEXT NOT NULL,
 value BLOB NOT NULL, keep_ms INTEGER NOT NULL CHECK(keep_ms >= 0),
 terminal INTEGER NOT NULL DEFAULT 0, legacy INTEGER NOT NULL DEFAULT 0,
 settled_at INTEGER, expires_at INTEGER,
 PRIMARY KEY(namespace, scope, id),
 CHECK((settled_at IS NULL) = (expires_at IS NULL))
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS receipts_expiry ON receipts(expires_at) WHERE expires_at IS NOT NULL;
CREATE TABLE IF NOT EXISTS receipt_references (
 namespace TEXT NOT NULL, scope TEXT NOT NULL, id TEXT NOT NULL,
 authority TEXT NOT NULL, epoch BLOB NOT NULL CHECK(length(epoch) = 8),
 sequence BLOB NOT NULL CHECK(length(sequence) = 8), acknowledged INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(namespace, scope, id, authority, epoch, sequence),
 FOREIGN KEY(namespace, scope, id) REFERENCES receipts(namespace, scope, id) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS receipt_pending_reference
 ON receipt_references(authority, epoch, sequence) WHERE acknowledged = 0;
CREATE TABLE IF NOT EXISTS receipt_watermarks (
 authority TEXT NOT NULL, epoch BLOB NOT NULL CHECK(length(epoch) = 8),
 sequence BLOB NOT NULL CHECK(length(sequence) = 8),
 PRIMARY KEY(authority, epoch)
) WITHOUT ROWID;
";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub namespace: String,
    pub scope: String,
    pub id: String,
}
impl Key {
    pub fn new(namespace: impl Into<String>, scope: impl Into<String>, id: impl Into<String>) -> Self {
        Self { namespace: namespace.into(), scope: scope.into(), id: id.into() }
    }
    pub fn bytes(&self) -> usize {
        self.namespace.len() + self.scope.len() + self.id.len()
    }
}

/// A transport sequence within one authority's epoch. Big-endian blobs retain
/// the complete unsigned wire range and SQLite's byte ordering matches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub authority: String,
    pub epoch: u64,
    pub sequence: u64,
}

pub fn get(connection: &Connection, key: &Key) -> Result<Option<Vec<u8>>> {
    Ok(connection
        .prepare_cached("SELECT value FROM receipts WHERE namespace=?1 AND scope=?2 AND id=?3")?
        .query_row(params![key.namespace, key.scope, key.id], |row| row.get(0))
        .optional()?)
}

/// Stores the latest result of one operation. `terminal` asserts that its
/// effect remains recognizable without this receipt; mere age is not proof.
/// Run together with that effect inside the same database transaction when
/// both belong in this database.
pub fn put(
    connection: &Connection,
    key: &Key,
    value: &[u8],
    keep: Duration,
    terminal: bool,
    reference: Option<&Reference>,
    now: i64,
) -> Result<()> {
    let keep = i64::try_from(keep.as_millis()).map_err(|_| Error::new("receipt retention is too long"))?;
    connection
        .prepare_cached(
            "INSERT INTO receipts(namespace,scope,id,value,keep_ms,terminal,legacy)
        VALUES(?1,?2,?3,?4,?5,?6,0)
        ON CONFLICT(namespace,scope,id) DO UPDATE SET value=excluded.value, keep_ms=excluded.keep_ms,
        terminal=MAX(receipts.terminal,excluded.terminal), legacy=0",
        )?
        .execute(params![key.namespace, key.scope, key.id, value, keep, terminal])?;
    if let Some(reference) = reference {
        let epoch = reference.epoch.to_be_bytes();
        let sequence = reference.sequence.to_be_bytes();
        connection.prepare_cached("INSERT INTO receipt_references(namespace,scope,id,authority,epoch,sequence,acknowledged)
            VALUES(?1,?2,?3,?4,?5,?6,EXISTS(SELECT 1 FROM receipt_watermarks WHERE authority=?4 AND epoch=?5 AND sequence>=?6))
            ON CONFLICT DO NOTHING")?
            .execute(params![key.namespace,key.scope,key.id,reference.authority,&epoch,&sequence])?;
        connection.prepare_cached("UPDATE receipts SET settled_at=NULL,expires_at=NULL
            WHERE namespace=?1 AND scope=?2 AND id=?3 AND terminal=0
            AND EXISTS(SELECT 1 FROM receipt_references r WHERE r.namespace=?1 AND r.scope=?2 AND r.id=?3 AND r.acknowledged=0)")?
            .execute(params![key.namespace,key.scope,key.id])?;
    }
    settle_if_proven(connection, key, now)?;
    Ok(())
}

/// Imports without overwriting a live result. A repeated legacy import must
/// match; a newer live result wins after runtime's normal identity checks.
/// The source file may be removed only after this transaction is committed.
/// A legacy file contains no durable acknowledgement position. Keeping the
/// receipt and acknowledging the hub were separate writes, and another copy
/// can still be queued elsewhere. Import preserves the original indefinite
/// deduplication until runtime supplies independent settlement evidence.
pub fn import(connection: &Connection, key: &Key, value: &[u8], keep: Duration, _now: i64) -> Result<()> {
    let keep = i64::try_from(keep.as_millis()).map_err(|_| Error::new("receipt retention is too long"))?;
    connection
        .prepare_cached(
            "INSERT INTO receipts(namespace,scope,id,value,keep_ms,terminal,legacy)
        VALUES(?1,?2,?3,?4,?5,0,1) ON CONFLICT DO NOTHING",
        )?
        .execute(params![key.namespace, key.scope, key.id, value, keep])?;
    let (legacy, stored): (bool, Vec<u8>) = connection
        .prepare_cached("SELECT legacy,value FROM receipts WHERE namespace=?1 AND scope=?2 AND id=?3")?
        .query_row(params![key.namespace, key.scope, key.id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    if legacy && stored != value {
        return Err(Error::new("legacy receipt copies disagree; both were preserved"));
    }
    Ok(())
}

/// Called only after the receiver's cumulative position was persisted and its
/// acknowledgement was sent. Only affected, still-unacknowledged references
/// are visited, independent of the number of old receipts.
pub fn acknowledge(connection: &Connection, through: &Reference, now: i64) -> Result<usize> {
    let epoch = through.epoch.to_be_bytes();
    let sequence = through.sequence.to_be_bytes();
    let keys = connection
        .prepare_cached(
            "SELECT DISTINCT namespace,scope,id FROM receipt_references
        WHERE authority=?1 AND epoch=?2 AND sequence<=?3 AND acknowledged=0",
        )?
        .query_map(params![through.authority, &epoch, &sequence], |row| {
            Ok(Key { namespace: row.get(0)?, scope: row.get(1)?, id: row.get(2)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    connection
        .prepare_cached(
            "INSERT INTO receipt_watermarks(authority,epoch,sequence) VALUES(?1,?2,?3)
        ON CONFLICT(authority,epoch) DO UPDATE SET sequence=MAX(receipt_watermarks.sequence,excluded.sequence)",
        )?
        .execute(params![through.authority, &epoch, &sequence])?;
    connection
        .prepare_cached(
            "UPDATE receipt_references SET acknowledged=1
        WHERE authority=?1 AND epoch=?2 AND sequence<=?3 AND acknowledged=0",
        )?
        .execute(params![through.authority, &epoch, &sequence])?;
    // A hub numbering in a new epoch no longer holds the old one: what it
    // numbered there is never sent again, so those places are retired with it.
    let retired = connection
        .prepare_cached(
            "SELECT DISTINCT namespace,scope,id FROM receipt_references
        WHERE authority=?1 AND epoch<>?2 AND acknowledged=0",
        )?
        .query_map(params![through.authority, &epoch], |row| {
            Ok(Key { namespace: row.get(0)?, scope: row.get(1)?, id: row.get(2)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    connection
        .prepare_cached(
            "UPDATE receipt_references SET acknowledged=1 WHERE authority=?1 AND epoch<>?2 AND acknowledged=0",
        )?
        .execute(params![through.authority, &epoch])?;
    // Acknowledged references carry their own proof. Old epoch watermarks are
    // no longer needed; a future copy in a retired epoch remains unproven until
    // that epoch is explicitly acknowledged again.
    connection
        .prepare_cached("DELETE FROM receipt_watermarks WHERE authority=?1 AND epoch<>?2")?
        .execute(params![through.authority, &epoch])?;
    let mut settled = 0;
    for key in keys.into_iter().chain(retired) {
        settled += settle_if_proven(connection, &key, now)?;
    }
    Ok(settled)
}

/// An independently durable terminal effect makes all transport copies harmless.
pub fn terminal(connection: &Connection, key: &Key, now: i64) -> Result<()> {
    connection
        .prepare_cached("UPDATE receipts SET terminal=1 WHERE namespace=?1 AND scope=?2 AND id=?3")?
        .execute(params![key.namespace, key.scope, key.id])?;
    settle_if_proven(connection, key, now)?;
    Ok(())
}

fn settle_if_proven(connection: &Connection, key: &Key, now: i64) -> Result<usize> {
    Ok(connection.prepare_cached("UPDATE receipts SET settled_at=?4,expires_at=?4+keep_ms
        WHERE namespace=?1 AND scope=?2 AND id=?3 AND settled_at IS NULL
        AND (terminal=1 OR (EXISTS(SELECT 1 FROM receipt_references r WHERE r.namespace=?1 AND r.scope=?2 AND r.id=?3)
        AND NOT EXISTS(SELECT 1 FROM receipt_references r WHERE r.namespace=?1 AND r.scope=?2 AND r.id=?3 AND r.acknowledged=0)))")?
        .execute(params![key.namespace,key.scope,key.id,now])?)
}

/// Deletes one bounded batch of proved, expired receipts. Cascading deletes
/// retire their transport references in the same transaction.
pub fn prune(connection: &Connection, now: i64, limit: usize) -> Result<usize> {
    Ok(connection
        .prepare_cached(
            "DELETE FROM receipts WHERE (namespace,scope,id) IN
        (SELECT namespace,scope,id FROM receipts WHERE expires_at<=?1 ORDER BY expires_at LIMIT ?2)",
        )?
        .execute(params![now, limit.clamp(1, 4096) as i64])?)
}

/// Conversation-scoped effects and inputs disappear with that conversation.
pub fn delete_scope(connection: &Connection, namespace: &str, scope: &str) -> Result<usize> {
    Ok(connection
        .prepare_cached("DELETE FROM receipts WHERE namespace=?1 AND scope=?2")?
        .execute(params![namespace, scope])?)
}
