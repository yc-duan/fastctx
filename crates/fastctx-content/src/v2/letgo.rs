//! A device letting go of what it may hold no more (design §2.3, §4.5):
//! what of its own the hub has not taken yet is kept to commit again first,
//! in the same transaction as what is let go, so nothing committed meanwhile
//! goes with the rest.

use super::Library;
use super::frame::Header;
use super::scopes::held;
use crate::Result;
use crate::error::invalid;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

/// How many frames are read at a time looking for a device's own tail.
const FRAMES_AT_ONCE: i64 = 64;

/// Every stream of `scope_id` in `holding` — the stream named `only`, when
/// one is — with this device's own frames past what the hub took of it
/// moved into the records waiting to be committed again. They are the
/// stream's tail, whoever orders the stream now: a successor known does not
/// make what this device wrote and never sent any less its own (S04).
fn keep_unsent(db: &Connection, scope_id: i64, holding: i64, device: &[u8; 32], only: Option<i64>) -> fastctx_storage::Result<()> {
    let streams: Vec<(i64, String)> = {
        let mut statement = db.prepare_cached("SELECT id, name FROM streams WHERE scope = ?1 AND (?2 IS NULL OR id = ?2) ORDER BY name")?;
        let rows = statement.query_map(params![scope_id, only], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for (stream, name) in streams {
        let uploaded: i64 =
            db.prepare_cached("SELECT uploaded FROM outbox WHERE stream = ?1")?.query_row([stream], |row| row.get(0)).optional()?.unwrap_or(0);
        // From the tail back, while the frames are this device's and the hub
        // has not taken them.
        let (mut start, mut below) = (None, i64::MAX);
        'tail: loop {
            let frames: Vec<(i64, Vec<u8>)> = {
                let mut statement = db.prepare_cached(
                    "SELECT first, header FROM frames WHERE stream = ?1 AND holding = ?2 AND first >= ?3 AND first < ?4
                     ORDER BY first DESC LIMIT ?5",
                )?;
                let rows = statement.query_map(params![stream, holding, uploaded, below, FRAMES_AT_ONCE], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            if frames.is_empty() {
                break;
            }
            for (first, header) in frames {
                if Header::decode(&header).map_err(invalid)?.device != *device {
                    break 'tail;
                }
                start = Some(first);
                below = first;
            }
        }
        if let Some(start) = start {
            super::rejected::rejected_in(db, scope_id, holding, stream, &name, start as u64)?;
        }
    }
    Ok(())
}

/// Whether `stream` of `scope` is one this device may hold no more in
/// `holding` (`withhold`): nothing is committed to it then.
pub(super) fn withheld(db: &Connection, scope: i64, stream: &str, holding: i64) -> fastctx_storage::Result<bool> {
    Ok(db
        .prepare_cached("SELECT 1 FROM withheld w JOIN streams s ON s.id = w.stream WHERE s.scope = ?1 AND s.name = ?2 AND w.holding = ?3")?
        .query_row(params![scope, stream, holding], |_| Ok(()))
        .optional()?
        .is_some())
}

impl Library {
    /// Lets `stream` of `scope` go, as one this device may hold no more:
    /// what of it is this device's own and not yet taken by the hub is kept
    /// to commit again, then the stream is emptied, in one transaction. From
    /// then nothing is committed to it in this holding period, a commit made
    /// meanwhile included, until the hub gives it back (`hold_again`).
    pub fn withhold(&self, scope: &str, stream: &str, device: &[u8; 32]) -> Result<()> {
        let (scope, stream, device) = (scope.to_string(), stream.to_string(), *device);
        self.database().write_now_at(Priority::Normal, 1 << 20, move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            // A stream not begun here yet is begun withheld: a commit on its
            // way finds it so (T04).
            let (stream_id, ..) = super::commit::stream_of(db, scope_id, &stream)?;
            keep_unsent(db, scope_id, holding, &device, Some(stream_id))?;
            super::window::truncate_in(db, scope_id, holding, stream_id, None)?;
            db.prepare_cached("INSERT OR IGNORE INTO withheld (stream, holding) VALUES (?1, ?2)")?.execute(params![stream_id, holding])?;
            Ok(())
        })?;
        self.wrote();
        Ok(())
    }

    /// The streams of `scope` withheld here in its holding period now.
    pub fn withheld_streams(&self, scope: &str) -> Result<std::collections::BTreeSet<String>> {
        let scope = scope.to_string();
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(Default::default()) };
            let mut statement =
                db.prepare_cached("SELECT s.name FROM withheld w JOIN streams s ON s.id = w.stream WHERE s.scope = ?1 AND w.holding = ?2")?;
            let rows = statement.query_map(params![scope_id, holding], |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?)
    }

    /// The hub gives `streams` of `scope` to this device to hold in the
    /// holding period `holding`: any of them withheld in that period is
    /// committed to again. A period ended meanwhile lifts nothing of the
    /// next one's (T05).
    pub fn hold_again(&self, scope: &str, holding: i64, streams: Vec<String>) -> Result<()> {
        let scope = scope.to_string();
        let any: bool = self.database().read_now(|db| Ok(db.prepare_cached("SELECT 1 FROM withheld LIMIT 1")?.query_row([], |_| Ok(())).optional()?.is_some()))?;
        if !any {
            return Ok(());
        }
        Ok(self.database().write_now_at(Priority::Normal, 64 + streams.len() * 64, move |db| {
            let Some((scope_id, _, current)) = held(db, &scope)? else { return Ok(()) };
            // What an ended period withheld goes with it.
            db.prepare_cached(
                "DELETE FROM withheld WHERE stream IN (SELECT id FROM streams WHERE scope = ?1) AND (?2 IS NULL OR holding <> ?2)",
            )?
            .execute(params![scope_id, current])?;
            if current != Some(holding) {
                return Ok(());
            }
            let mut lift = db.prepare_cached(
                "DELETE FROM withheld WHERE holding = ?3 AND stream IN (SELECT id FROM streams WHERE scope = ?1 AND name = ?2)",
            )?;
            for stream in &streams {
                lift.execute(params![scope_id, stream, holding])?;
            }
            Ok(())
        })?)
    }

    /// Revokes `scope` here as `revoke` does, what this device's own streams
    /// hold that the hub has not taken kept first to commit again, in the
    /// same transaction: when the period's purge comes, it is kept for its
    /// user (§3, §4.5).
    pub fn revoke_keeping(&self, scope: &str, device: &[u8; 32]) -> Result<()> {
        let (scope, device) = (scope.to_string(), *device);
        self.database().write_now_at(Priority::Control, 1 << 20, move |db| revoke_keeping_in(db, &scope, &device))?;
        self.wrote();
        Ok(())
    }
}

/// `Library::revoke_keeping` in the transaction `db` is in.
pub(super) fn revoke_keeping_in(db: &Connection, scope: &str, device: &[u8; 32]) -> fastctx_storage::Result<()> {
    if let Some((scope_id, _, Some(holding))) = held(db, scope)? {
        keep_unsent(db, scope_id, holding, device, None)?;
    }
    super::scopes::revoke_in(db, scope)
}
