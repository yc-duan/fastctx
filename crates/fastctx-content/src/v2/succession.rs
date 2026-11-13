//! A holder's side of a stream's succession (§2.2, §7.4, §7.5, §8): it
//! takes frames only from the epochs the hub signed, only from each
//! epoch's device and never past where a handover cut it; it skips a
//! predecessor's tail on a signed gap fact; and it tells the hub where each
//! stream stands, cutting back to the last place the hub agrees with when
//! they differ.
//!
//! The hub's side of a cursor: it agrees with a device's (sequence, chain
//! value) when one of its frames, or a gap fact, ends there with that
//! value; otherwise it answers the ends of its frames before that sequence
//! for the device to find the last one they share.

use super::Library;
use super::facts::{EpochFact, GapFact};
use super::frame::Header;
use super::handover::Signed;
use super::scopes::held;
use crate::error::invalid;
use crate::{Error, Result};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

/// Checks a fact against the hub's service key, pinned when the hub was
/// first met (§6.1).
pub trait HubVerifier: Send + Sync {
    fn verify(&self, fact: &[u8], signature: &[u8]) -> bool;
}

/// Ends of the hub's frames answered at once for a device to find where
/// it last agreed.
pub const ENDS_AT_ONCE: usize = 64;

/// What the hub makes of a device's cursor on one stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// The hub holds that place with that chain value.
    Agrees,
    /// It does not: the ends of the hub's frames (or gaps) at or before
    /// the device's sequence, latest first, for the device to find the
    /// last one it shares; an empty list when the hub has nothing before
    /// it.
    Differs(Vec<(u64, [u8; 32])>),
}

fn stream_of(db: &Connection, scope: i64, name: &str) -> fastctx_storage::Result<i64> {
    if let Some(id) = db
        .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
        .query_row(params![scope, name], |row| row.get(0))
        .optional()?
    {
        return Ok(id);
    }
    db.prepare_cached("INSERT INTO streams (scope, name, epoch, next, head, complete_through) VALUES (?1, ?2, 0, 0, NULL, NULL)")?
        .execute(params![scope, name])?;
    // A stream made, again or for the first time, has an era of its own (JJ01).
    let made = db.last_insert_rowid();
    super::view::rebased(db, made)?;
    Ok(made)
}

/// The descriptor of `epoch` of `stream` the holder took, if any.
pub(super) fn known_epoch(db: &Connection, stream: i64, epoch: u64) -> fastctx_storage::Result<Option<EpochFact>> {
    let fact: Option<Vec<u8>> = db
        .prepare_cached("SELECT fact FROM known_epochs WHERE stream = ?1 AND epoch = ?2")?
        .query_row(params![stream, epoch as i64], |row| row.get(0))
        .optional()?;
    fact.map(|fact| EpochFact::decode(&fact).map_err(invalid)).transpose()
}

/// Whether a holder may import `header`: its epoch's descriptor is here,
/// names the frame's device and key, the frame starts within the epoch,
/// and no later epoch cut it before the frame ends.
pub(super) fn may_import(db: &Connection, stream: i64, header: &Header) -> fastctx_storage::Result<std::result::Result<(), u64>> {
    let Some(epoch) = known_epoch(db, stream, header.epoch)? else { return Ok(Err(header.epoch)) };
    if epoch.device != header.device || epoch.key != header.key {
        return Err(invalid("the frame is not of its epoch's device"));
    }
    if header.first < epoch.start {
        return Err(invalid("the frame starts before its epoch"));
    }
    let successor: Option<Vec<u8>> = db
        .prepare_cached("SELECT fact FROM known_epochs WHERE stream = ?1 AND predecessor = ?2")?
        .query_row(params![stream, header.epoch as i64], |row| row.get(0))
        .optional()?;
    if let Some(successor) = successor {
        let successor = EpochFact::decode(&successor).map_err(invalid)?;
        let end = header.first + u64::from(header.count);
        if end > successor.start {
            return Err(invalid("the frame goes past where a handover cut its epoch"));
        }
        if end == successor.start && header.chain() != successor.follows {
            return Err(invalid("the frame does not end where a handover cut its epoch"));
        }
    }
    Ok(Ok(()))
}

/// The chain value this holder has at the end `end` of `stream` in
/// `holding`: where a frame of it ends, or where a gap the hub signed ends
/// that follows what it holds.
pub(super) fn held_end(db: &Connection, stream: i64, holding: i64, end: u64) -> fastctx_storage::Result<Option<[u8; 32]>> {
    // The gaps ending at `end` and back before it, to a frame or the start.
    let known: i64 = db.prepare_cached("SELECT COUNT(*) FROM known_gaps WHERE stream = ?1")?.query_row([stream], |row| row.get(0))?;
    let mut gaps = Vec::new();
    let mut at = end;
    let mut value = loop {
        if let Some(head) = held_frame(db, stream, holding, at)? {
            break head;
        }
        let Some(gap) = gap_ending(db, stream, at)? else { return Ok(None) };
        let from = gap.from;
        gaps.push(gap);
        if from == 0 {
            break [0; 32];
        }
        if gaps.len() as i64 > known {
            return Ok(None);
        }
        at = from - 1;
    };
    for gap in gaps.iter().rev() {
        if gap.start != value {
            return Ok(None);
        }
        value = gap.follows;
    }
    Ok(Some(value))
}

fn held_frame(db: &Connection, stream: i64, holding: i64, end: u64) -> fastctx_storage::Result<Option<[u8; 32]>> {
    let head: Option<Vec<u8>> = db
        .prepare_cached("SELECT head FROM frames WHERE stream = ?1 AND holding = ?2 AND first + count - 1 = ?3")?
        .query_row(params![stream, holding, end as i64], |row| row.get(0))
        .optional()?;
    head.map(|head| head.try_into().map_err(|_| invalid("a malformed chain value"))).transpose()
}

fn gap_ending(db: &Connection, stream: i64, end: u64) -> fastctx_storage::Result<Option<GapFact>> {
    let mut statement = db.prepare_cached("SELECT fact FROM known_gaps WHERE stream = ?1 AND from_sequence <= ?2")?;
    let facts = statement.query_map(params![stream, end as i64], |row| row.get::<_, Vec<u8>>(0))?;
    for fact in facts {
        let gap = GapFact::decode(&fact?).map_err(invalid)?;
        if gap.to == end {
            return Ok(Some(gap));
        }
    }
    Ok(None)
}

/// Moves the cursor at `next` with chain value `head` past each gap fact
/// that starts there with that value; where it stands then.
pub(super) fn skip_gaps(
    db: &Connection,
    stream: i64,
    holding: i64,
    mut at: (u64, [u8; 32]),
) -> fastctx_storage::Result<(u64, [u8; 32])> {
    loop {
        let gap: Option<Vec<u8>> = db
            .prepare_cached("SELECT fact FROM known_gaps WHERE stream = ?1 AND from_sequence = ?2")?
            .query_row(params![stream, at.0 as i64], |row| row.get(0))
            .optional()?;
        let Some(gap) = gap else { return Ok(at) };
        let gap = GapFact::decode(&gap).map_err(invalid)?;
        if gap.start != at.1 {
            return Ok(at);
        }
        db.prepare_cached(
            "INSERT INTO cursors (stream, holding, sequence, head) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (stream, holding) DO UPDATE SET sequence = excluded.sequence, head = excluded.head",
        )?
        .execute(params![stream, holding, gap.to as i64, &gap.follows[..]])?;
        db.prepare_cached("UPDATE streams SET next = MAX(next, ?2), head = ?3 WHERE id = ?1")?
            .execute(params![stream, gap.to as i64 + 1, &gap.follows[..]])?;
        at = (gap.to + 1, gap.follows);
    }
}

impl Library {
    /// Takes an epoch descriptor of `scope` the hub signed (§2.2). The same
    /// again is nothing; another for the same epoch is refused.
    pub fn accept_epoch(&self, scope: &str, signed: Signed, verifier: &dyn HubVerifier) -> Result<()> {
        if !verifier.verify(&signed.fact, &signed.signature) {
            return Err(Error::Invalid("the epoch's descriptor is not the hub's".into()));
        }
        let fact = EpochFact::decode(&signed.fact).map_err(Error::Invalid)?;
        if fact.scope != scope {
            return Err(Error::Invalid("the epoch is of another scope".into()));
        }
        let scope = scope.to_string();
        Ok(self.database().write_relaxed_now_at(Priority::Normal, 512, move |db| {
            let Some((scope_id, _, _)) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let stream = stream_of(db, scope_id, &fact.stream)?;
            if let Some(kept) = known_epoch(db, stream, fact.epoch)? {
                return if kept == fact { Ok(()) } else { Err(invalid("another descriptor for that epoch is here")) };
            }
            db.prepare_cached(
                "INSERT INTO known_epochs (stream, epoch, predecessor, fact, signature) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![stream, fact.epoch as i64, fact.predecessor.map(|epoch| epoch as i64), signed.fact, signed.signature])?;
            Ok(())
        })?)
    }

    /// Takes a gap fact of `scope` the hub signed (§8): a cursor standing
    /// where it starts skips to its end, now or when it gets there.
    pub fn accept_gap(&self, scope: &str, signed: Signed, verifier: &dyn HubVerifier) -> Result<()> {
        if !verifier.verify(&signed.fact, &signed.signature) {
            return Err(Error::Invalid("the gap fact is not the hub's".into()));
        }
        let fact = GapFact::decode(&signed.fact).map_err(Error::Invalid)?;
        if fact.scope != scope {
            return Err(Error::Invalid("the gap is of another scope".into()));
        }
        let scope = scope.to_string();
        Ok(self.database().write_relaxed_now_at(Priority::Normal, 512, move |db| {
            let Some((scope_id, _, holding)) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let stream = stream_of(db, scope_id, &fact.stream)?;
            db.prepare_cached(
                "INSERT OR IGNORE INTO known_gaps (stream, epoch, from_sequence, fact, signature) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![stream, fact.epoch as i64, fact.from as i64, signed.fact, signed.signature])?;
            if let Some(holding) = holding {
                let cursor: Option<(i64, Option<Vec<u8>>)> = db
                    .prepare_cached("SELECT sequence, head FROM cursors WHERE stream = ?1 AND holding = ?2")?
                    .query_row(params![stream, holding], |row| Ok((row.get(0)?, row.get(1)?)))
                    .optional()?;
                let at = match cursor {
                    Some((sequence, head)) => {
                        (sequence as u64 + 1, head.unwrap_or_default().try_into().map_err(|_| invalid("a malformed cursor"))?)
                    }
                    None => (0, [0; 32]),
                };
                skip_gaps(db, stream, holding, at)?;
            }
            Ok(())
        })?)
    }

    /// Where each stream of `scope` stands here in the current period: its
    /// name, the last sequence applied and the chain value there, for the
    /// hub to check (§6.6, §7.4).
    pub fn standing(&self, scope: &str) -> Result<Vec<(String, u64, [u8; 32])>> {
        let scope = scope.to_string();
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
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
                let head = head.unwrap_or_default().try_into().map_err(|_| invalid("a malformed cursor"))?;
                standing.push((name, sequence as u64, head));
            }
            Ok(standing)
        })?)
    }

    /// The hub's word on `device`'s cursor at `sequence` with chain value
    /// `head` on `stream` of `scope`, in the device's holding period
    /// `holding`; an agreeing one is kept as the device's progress, shown
    /// as how many devices have caught up, never as proof of what they hold
    /// (§6.6).
    pub fn check_cursor(
        &self,
        device: &[u8; 32],
        scope: &str,
        stream: &str,
        position: (i64, u64, [u8; 32]),
    ) -> Result<Standing> {
        let (scope, stream, device) = (scope.to_string(), stream.to_string(), *device);
        let (holding, sequence, head) = position;
        Ok(self.database().write_relaxed_now_at(Priority::Background, 256, move |db| {
            let Some((scope_id, _, Some(own))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let Some(stream_id) = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| row.get::<_, i64>(0))
                .optional()?
            else {
                return Ok(Standing::Differs(Vec::new()));
            };
            let at_frame: Option<Vec<u8>> = db
                .prepare_cached("SELECT head FROM frames WHERE stream = ?1 AND holding = ?2 AND first + count - 1 = ?3")?
                .query_row(params![stream_id, own, sequence as i64], |row| row.get(0))
                .optional()?;
            let at_gap: Option<Vec<u8>> = db
                .prepare_cached("SELECT follows FROM gaps WHERE stream = ?1 AND to_sequence = ?2")?
                .query_row(params![stream_id, sequence as i64], |row| row.get(0))
                .optional()?;
            if at_frame.as_deref() == Some(&head[..]) || at_gap.as_deref() == Some(&head[..]) {
                db.prepare_cached(
                    "INSERT INTO device_cursors (device, stream, holding, sequence) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (device, stream, holding) DO UPDATE SET sequence = excluded.sequence",
                )?
                .execute(params![&device[..], stream_id, holding, sequence as i64])?;
                return Ok(Standing::Agrees);
            }
            let mut ends = Vec::new();
            let mut statement = db.prepare_cached(
                "SELECT end, head FROM (
                    SELECT first + count - 1 AS end, head FROM frames WHERE stream = ?1 AND holding = ?2 AND first + count - 1 <= ?3
                    UNION ALL
                    SELECT to_sequence AS end, follows AS head FROM gaps WHERE stream = ?1 AND to_sequence <= ?3)
                 ORDER BY end DESC LIMIT ?4",
            )?;
            let rows = statement.query_map(params![stream_id, own, sequence as i64, ENDS_AT_ONCE as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            for row in rows {
                let (end, head) = row?;
                ends.push((end as u64, head.try_into().map_err(|_| invalid("a malformed chain value"))?));
            }
            Ok(Standing::Differs(ends))
        })?)
    }

    /// The last of the hub's `ends` (latest first) this replica holds with
    /// the same chain value, to cut `stream` of `scope` back to; `None`
    /// when none is shared.
    pub fn shared_end(&self, scope: &str, stream: &str, ends: &[(u64, [u8; 32])]) -> Result<Option<u64>> {
        let (scope, stream, ends) = (scope.to_string(), stream.to_string(), ends.to_vec());
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, Some(holding))) = held(db, &scope)? else { return Ok(None) };
            let Some(stream_id) = db
                .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
                .query_row(params![scope_id, stream], |row| row.get::<_, i64>(0))
                .optional()?
            else {
                return Ok(None);
            };
            for (end, head) in ends {
                if held_end(db, stream_id, holding, end)? == Some(head) {
                    return Ok(Some(end));
                }
            }
            Ok(None)
        })?)
    }
}
