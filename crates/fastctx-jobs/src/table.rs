//! The table of jobs, and what a transaction does to it.

use crate::Job;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use fastctx_storage::{Error, ErrorKind, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// The longest key or unit of a job.
pub const KEY_BYTES: usize = 256;
/// The largest payload; larger input is named by where it is.
pub const PAYLOAD_BYTES: usize = 64 * 1024;

pub(crate) const WAITING: i64 = 0;
pub(crate) const RUNNING: i64 = 1;
pub(crate) const ASIDE: i64 = 2;

/// A service adds this to the schema of the database its jobs live in.
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS jobs (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    unit TEXT NOT NULL,
    payload BLOB NOT NULL,
    state INTEGER NOT NULL DEFAULT 0,
    due INTEGER NOT NULL,
    tries INTEGER NOT NULL DEFAULT 0,
    again BLOB,
    failure TEXT,
    UNIQUE (kind, key)
);
CREATE INDEX IF NOT EXISTS jobs_due ON jobs (kind, state, due);
CREATE INDEX IF NOT EXISTS jobs_unit ON jobs (unit);
CREATE INDEX IF NOT EXISTS jobs_order ON jobs (kind, unit, state, id);
";

pub(crate) fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as i64)
}

/// Asks for work, in the transaction that records why it is needed. A job
/// of the same kind and key that waits takes this payload and is due now;
/// one that runs is run once more with it after.
pub fn submit(db: &Connection, job: &Job) -> Result<()> {
    if job.key.is_empty() || job.key.len() > KEY_BYTES || job.unit.len() > KEY_BYTES {
        return Err(Error::with_kind(
            ErrorKind::Invalid,
            format!("a job's key and unit are at most {KEY_BYTES} bytes"),
        ));
    }
    if job.payload.len() > PAYLOAD_BYTES {
        return Err(Error::with_kind(
            ErrorKind::Invalid,
            format!("a job's payload is at most {PAYLOAD_BYTES} bytes; larger input is named by where it is"),
        ));
    }
    let known: Option<(i64, i64)> = db
        .prepare_cached("SELECT id, state FROM jobs WHERE kind = ?1 AND key = ?2")?
        .query_row(params![job.kind, job.key], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    match known {
        None => {
            db.prepare_cached("INSERT INTO jobs (kind, key, unit, payload, due) VALUES (?1, ?2, ?3, ?4, ?5)")?
                .execute(params![job.kind, job.key, job.unit, job.payload, now()])?;
        }
        Some((id, RUNNING)) => {
            db.prepare_cached("UPDATE jobs SET again = ?2 WHERE id = ?1")?.execute(params![id, job.payload])?;
        }
        Some((id, _)) => {
            db.prepare_cached(
                "UPDATE jobs SET payload = ?2, state = 0, due = ?3, tries = 0, failure = NULL, again = NULL \
                 WHERE id = ?1",
            )?
            .execute(params![id, job.payload, now()])?;
        }
    }
    Ok(())
}

/// Makes a kind's waiting jobs due now: what they wait for besides time,
/// such as a way to another machine, may be there. `Jobs::stir` then has
/// them looked at.
pub fn rouse(db: &Connection, kind: &str) -> Result<usize> {
    Ok(db.prepare_cached("UPDATE jobs SET due = ?2 WHERE kind = ?1 AND state = ?3 AND due > ?2")?.execute(params![
        kind,
        now(),
        WAITING
    ])?)
}

/// Takes back a unit's jobs of a kind that have not begun, and names them:
/// what asked for them is told they were not done. One that runs goes on.
pub fn withdraw(db: &Connection, kind: &str, unit: &str) -> Result<Vec<String>> {
    let mut withdrawn =
        db.prepare_cached("DELETE FROM jobs WHERE kind = ?1 AND unit = ?2 AND state = ?3 RETURNING key")?;
    let keys = withdrawn
        .query_map(params![kind, unit, WAITING], |row| row.get(0))?
        .collect::<fastctx_storage::rusqlite::Result<_>>()?;
    Ok(keys)
}

/// Drops a unit's jobs, in the transaction that ends the unit. One running
/// then finishes into nothing. Where a dropped job was to settle an
/// operation, the operation is settled with it, so that whoever asked is
/// told the work is not to be, and is not left asking after it.
pub fn forget(db: &Connection, unit: &str) -> Result<usize> {
    let dropped: Vec<(String, String, i64)> = db
        .prepare_cached("DELETE FROM jobs WHERE unit = ?1 RETURNING kind, key, state")?
        .query_map([unit], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<fastctx_storage::rusqlite::Result<_>>()?;
    for (kind, key, state) in &dropped {
        crate::operations::dropped(db, kind, key, *state == RUNNING)?;
    }
    Ok(dropped.len())
}
