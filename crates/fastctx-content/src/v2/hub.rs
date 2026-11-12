//! A hub's owner library taking frames (§6.5): kept, applied and published
//! as three steps.
//!
//! A frame is checked before any transaction: its signature, its records,
//! and every object it refers to present here, with the ones its source
//! device could not read through the gate proven held by their whole bytes
//! (P01, Q01, Q02; the proof itself is the caller's exchange). Then:
//! - one that follows the stream's applied prefix is authorized against
//!   that prefix by its kind's rules, applied with a new change number of
//!   its scope, and answered `applied`; frames parked behind it that now
//!   follow are applied in turn;
//! - one of a later epoch, whose predecessor's tail is not all here, is
//!   kept unapplied, with the scope's parked sequence and accumulator, and
//!   answered `parked`; nothing reads it;
//! - one refused at application, and every later frame of its epoch, is
//!   answered `rejected` from then on.
//!
//! The same frame again is answered as it was.

use super::Library;
use super::exchange::{RawFrame, Verifier};
use super::frame::{Header, KindRecord, open};
use super::items::{Applying, IS, apply};
use super::objects::present;
use super::reads::Entry;
use super::scopes::held;
use crate::error::invalid;
use crate::record::{ObjectRef, State, unpack};
use crate::{Error, Result};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Judges whether a record may change what it changes, against the item as
/// the applied prefix has it (§11): the same rules its producer judged by.
pub trait Authorizer: Send + Sync {
    fn authorize(&self, record: &KindRecord, before: Option<&Entry>) -> std::result::Result<(), String>;
}

/// Signs the hub's word on a frame with its service key (§6.1); the signer
/// binds the hub's chain head as it stands.
pub trait HubSigner: Send + Sync {
    fn sign(&self, statement: &[u8]) -> Vec<u8>;
}

/// How the source's hold on the objects stands: those it may read through
/// the gate, and those it proved.
pub struct Holding<'a> {
    pub readable: &'a (dyn Fn(&ObjectRef) -> bool + Sync),
    pub proven: &'a [[u8; 32]],
}

/// The hub's word on a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Received {
    Applied { change: u64, prefix: [u8; 32], signature: Vec<u8>, drained: Vec<u64> },
    Parked { sequence: u64, accumulated: [u8; 32], signature: Vec<u8> },
    Rejected { why: String, signature: Vec<u8> },
    /// Not held yet: send `from` to `to` first.
    Gap { from: u64, to: u64 },
    /// Not here yet: put these objects first.
    Missing(Vec<ObjectRef>),
    /// Prove holding these by their whole bytes first.
    Prove(Vec<ObjectRef>),
}

/// What a receipt says, as signed: its kind, the frame, and where it stands.
fn statement(kind: &str, header: &Header, fields: &[&[u8]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for part in [kind.as_bytes(), header.scope.as_bytes(), header.stream.as_bytes()] {
        bytes.extend_from_slice(&(part.len() as u32).to_le_bytes());
        bytes.extend_from_slice(part);
    }
    bytes.extend_from_slice(&header.epoch.to_le_bytes());
    bytes.extend_from_slice(&header.first.to_le_bytes());
    bytes.extend_from_slice(&header.digest);
    for field in fields {
        bytes.extend_from_slice(&(field.len() as u32).to_le_bytes());
        bytes.extend_from_slice(field);
    }
    bytes
}

fn objects_of(records: &[KindRecord]) -> Vec<ObjectRef> {
    let mut seen = BTreeSet::new();
    let mut objects = Vec::new();
    for record in records {
        if let State::Is { objects: referred, .. } = &record.record.state {
            for object in referred {
                if seen.insert(object.hash) {
                    objects.push(*object);
                }
            }
        }
    }
    objects
}

/// The item as the applied prefix has it.
fn before(db: &Connection, stream: i64, holding: i64, item: &[u8]) -> fastctx_storage::Result<Option<Entry>> {
    let found = db
        .prepare_cached(
            "SELECT item, place, revision, kind, schema, body, objects FROM items
             WHERE stream = ?1 AND item = ?2 AND holding = ?3 AND state = ?4",
        )?
        .query_row(params![stream, item, holding, IS], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, u16>(3)?,
                row.get::<_, u16>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
                row.get::<_, Option<Vec<u8>>>(6)?,
            ))
        })
        .optional()?;
    let Some((item, order, revision, kind, schema, body, objects)) = found else { return Ok(None) };
    Ok(Some(Entry {
        item,
        order,
        revision: revision as u64,
        kind,
        schema,
        body: body.unwrap_or_default(),
        objects: unpack(&objects.unwrap_or_default()).map_err(invalid)?,
    }))
}

/// The applied prefix of a stream: its id, epoch, next sequence and chain
/// value.
struct Prefix {
    stream: i64,
    epoch: u64,
    next: u64,
    head: [u8; 32],
}

fn prefix(db: &Connection, scope: i64, holding: i64, name: &str) -> fastctx_storage::Result<Prefix> {
    let found: Option<(i64, i64)> = db
        .prepare_cached("SELECT id, epoch FROM streams WHERE scope = ?1 AND name = ?2")?
        .query_row(params![scope, name], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let (stream, epoch) = match found {
        Some(found) => found,
        None => {
            db.prepare_cached("INSERT INTO streams (scope, name, epoch, next, head, complete_through) VALUES (?1, ?2, 0, 0, NULL, NULL)")?
                .execute(params![scope, name])?;
            (db.last_insert_rowid(), 0)
        }
    };
    let cursor: Option<(i64, Option<Vec<u8>>)> = db
        .prepare_cached("SELECT sequence, head FROM cursors WHERE stream = ?1 AND holding = ?2")?
        .query_row(params![stream, holding], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let (next, head) = match cursor {
        Some((sequence, head)) => {
            (sequence as u64 + 1, head.unwrap_or_default().try_into().map_err(|_| invalid("a malformed cursor"))?)
        }
        None => (0, [0; 32]),
    };
    Ok(Prefix { stream, epoch: epoch as u64, next, head })
}

/// One frame to apply: as it came, its header and its records.
struct Arriving<'a> {
    frame: &'a RawFrame,
    header: &'a Header,
    records: &'a [KindRecord],
}

/// Applies one frame that follows the applied prefix, or refuses it; the
/// change number it took, or why it was refused.
fn apply_frame(
    db: &Connection,
    scope: i64,
    holding: i64,
    at: &mut Prefix,
    arriving: Arriving<'_>,
    authorizer: &dyn Authorizer,
) -> fastctx_storage::Result<std::result::Result<(u64, [u8; 32]), String>> {
    let Arriving { frame, header, records } = arriving;
    for record in records {
        let prior = before(db, at.stream, holding, &record.record.item)?;
        if let Err(why) = authorizer.authorize(record, prior.as_ref()) {
            return Ok(Err(why));
        }
    }
    let head = header.chain();
    db.prepare_cached(
        "INSERT INTO frames (stream, epoch, first, holding, count, head, header, signature, body, change)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
    )?
    .execute(params![
        at.stream,
        header.epoch as i64,
        header.first as i64,
        holding,
        header.count,
        &head[..],
        frame.header,
        frame.signature,
        frame.body
    ])?;
    let mut referring =
        db.prepare_cached("INSERT OR IGNORE INTO frame_objects (stream, epoch, first, holding, hash) VALUES (?1, ?2, ?3, ?4, ?5)")?;
    for object in objects_of(records) {
        referring.execute(params![at.stream, header.epoch as i64, header.first as i64, holding, &object.hash[..]])?;
    }
    let applying = Applying { scope, stream: at.stream, holding };
    for (offset, record) in records.iter().enumerate() {
        apply(db, applying, header.first + offset as u64, record)?;
    }
    let last: Option<(i64, Vec<u8>)> = db
        .prepare_cached("SELECT change, prefix FROM change_log WHERE scope = ?1 ORDER BY change DESC LIMIT 1")?
        .query_row([scope], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let (change, previous) = match last {
        Some((change, prefix)) => (change as u64 + 1, prefix),
        None => (1, vec![0; 32]),
    };
    let mut hasher = Sha256::new();
    hasher.update(b"fastctx-change-prefix/1");
    hasher.update(&previous);
    hasher.update(at.stream.to_le_bytes());
    hasher.update(header.epoch.to_le_bytes());
    hasher.update(header.first.to_le_bytes());
    hasher.update(head);
    let prefix: [u8; 32] = hasher.finalize().into();
    db.prepare_cached("INSERT INTO change_log (scope, change, stream, epoch, first, prefix) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?
        .execute(params![scope, change as i64, at.stream, header.epoch as i64, header.first as i64, &prefix[..]])?;
    db.prepare_cached("UPDATE frames SET change = ?5 WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
        .execute(params![at.stream, header.epoch as i64, header.first as i64, holding, change as i64])?;
    let end = header.first + u64::from(header.count);
    db.prepare_cached(
        "INSERT INTO cursors (stream, holding, sequence, head) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (stream, holding) DO UPDATE SET sequence = excluded.sequence, head = excluded.head",
    )?
    .execute(params![at.stream, holding, end as i64 - 1, &head[..]])?;
    db.prepare_cached("UPDATE streams SET next = ?2, head = ?3, complete_through = ?4 WHERE id = ?1")?
        .execute(params![at.stream, end as i64, &head[..], end as i64 - 1])?;
    at.next = end;
    at.head = head;
    Ok(Ok((change, prefix)))
}

/// Refuses a frame and every later frame of its epoch, parked or still to
/// come.
fn refuse(db: &Connection, stream: i64, header: &Header, why: &str, signature: &[u8]) -> fastctx_storage::Result<()> {
    db.prepare_cached(
        "INSERT OR REPLACE INTO refused_frames (stream, epoch, first, digest, why, signature) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?
    .execute(params![stream, header.epoch as i64, header.first as i64, &header.digest[..], why, signature])?;
    for table in ["parked_objects", "parked"] {
        db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND epoch = ?2 AND first > ?3"))?
            .execute(params![stream, header.epoch as i64, header.first as i64])?;
    }
    Ok(())
}

impl Library {
    /// Takes one frame of `scope` as a hub.
    pub fn take(
        &self,
        scope: &str,
        frame: RawFrame,
        verifier: &dyn Verifier,
        holding: &Holding<'_>,
        authorizer: std::sync::Arc<dyn Authorizer>,
        signer: std::sync::Arc<dyn HubSigner>,
    ) -> Result<Received> {
        let header = Header::decode(&frame.header).map_err(Error::Invalid)?;
        if header.scope != scope {
            return Err(Error::Invalid("the frame is of another scope".into()));
        }
        if !verifier.verify(&header, &frame.header, &frame.signature) {
            return Err(Error::Invalid("the frame's signature does not verify".into()));
        }
        let records = open(&header, &frame.body).map_err(Error::Invalid)?;
        for record in &records {
            record.record.check().map_err(Error::Invalid)?;
        }
        let objects = objects_of(&records);
        let (mut missing, mut unproven) = (Vec::new(), Vec::new());
        for object in &objects {
            match self.size(&object.hash)? {
                Some(size) if size == object.size => {
                    if !(holding.readable)(object) && !holding.proven.contains(&object.hash) {
                        unproven.push(*object);
                    }
                }
                Some(_) => return Err(Error::Invalid("an object of that hash is here with another length".into())),
                None => missing.push(*object),
            }
        }
        if !missing.is_empty() {
            return Ok(Received::Missing(missing));
        }
        if !unproven.is_empty() {
            return Ok(Received::Prove(unproven));
        }
        let scope = scope.to_string();
        let bytes = frame.body.len() + frame.header.len() + 1024 + records.len() * 64;
        Ok(self.database().write_now_at(Priority::Normal, bytes, move |db| {
            let Some((scope_id, state, Some(holding_id))) = held(db, &scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" {
                return Err(invalid("the scope is not live"));
            }
            for object in &objects {
                if present(db, &object.hash)?.is_none() {
                    return Err(invalid("an object went while the frame was checked"));
                }
            }
            let mut at = prefix(db, scope_id, holding_id, &header.stream)?;
            let refused: Option<(String, Vec<u8>)> = db
                .prepare_cached(
                    "SELECT why, signature FROM refused_frames WHERE stream = ?1 AND epoch = ?2
                     AND (first = ?3 OR first < ?3) ORDER BY first DESC LIMIT 1",
                )?
                .query_row(params![at.stream, header.epoch as i64, header.first as i64], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            if let Some((why, signature)) = refused {
                return Ok(Received::Rejected { why, signature });
            }
            let applied: Option<(Vec<u8>, Option<i64>)> = db
                .prepare_cached("SELECT header, change FROM frames WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
                .query_row(params![at.stream, header.epoch as i64, header.first as i64, holding_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            if let Some((known, change)) = applied {
                if known != frame.header {
                    return Err(invalid("another frame is applied at that position"));
                }
                let change = change.unwrap_or_default() as u64;
                let prefix: Vec<u8> = db
                    .prepare_cached("SELECT prefix FROM change_log WHERE scope = ?1 AND change = ?2")?
                    .query_row(params![scope_id, change as i64], |row| row.get(0))?;
                let prefix: [u8; 32] = prefix.try_into().map_err(|_| invalid("a malformed prefix"))?;
                let signature = signer.sign(&statement("applied", &header, &[&change.to_le_bytes(), &prefix]));
                return Ok(Received::Applied { change, prefix, signature, drained: Vec::new() });
            }
            let parked: Option<(Vec<u8>, i64)> = db
                .prepare_cached("SELECT header, sequence FROM parked WHERE stream = ?1 AND epoch = ?2 AND first = ?3")?
                .query_row(params![at.stream, header.epoch as i64, header.first as i64], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            if let Some((known, sequence)) = parked {
                if known != frame.header {
                    return Err(invalid("another frame is parked at that position"));
                }
                let accumulated: Vec<u8> = db
                    .prepare_cached("SELECT accumulated FROM parked_history WHERE scope = ?1 AND sequence = ?2")?
                    .query_row(params![scope_id, sequence], |row| row.get(0))?;
                let accumulated: [u8; 32] = accumulated.try_into().map_err(|_| invalid("a malformed accumulator"))?;
                let signature = signer.sign(&statement("parked", &header, &[&(sequence as u64).to_le_bytes(), &accumulated]));
                return Ok(Received::Parked { sequence: sequence as u64, accumulated, signature });
            }
            if header.epoch > at.epoch {
                // A successor's frame before its predecessor's tail: kept,
                // not applied, not published (§8).
                let last: Option<(i64, Vec<u8>)> = db
                    .prepare_cached("SELECT sequence, accumulated FROM parked_history WHERE scope = ?1 ORDER BY sequence DESC LIMIT 1")?
                    .query_row([scope_id], |row| Ok((row.get(0)?, row.get(1)?)))
                    .optional()?;
                let (sequence, previous) = match last {
                    Some((sequence, accumulated)) => (sequence as u64 + 1, accumulated),
                    None => (1, vec![0; 32]),
                };
                let mut hasher = Sha256::new();
                hasher.update(b"fastctx-parked/1");
                hasher.update(&previous);
                hasher.update(header.encode());
                hasher.update(header.digest);
                let accumulated: [u8; 32] = hasher.finalize().into();
                db.prepare_cached(
                    "INSERT INTO parked (stream, epoch, first, header, signature, body, sequence) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )?
                .execute(params![
                    at.stream,
                    header.epoch as i64,
                    header.first as i64,
                    frame.header,
                    frame.signature,
                    frame.body,
                    sequence as i64
                ])?;
                let mut referring =
                    db.prepare_cached("INSERT OR IGNORE INTO parked_objects (stream, epoch, first, hash) VALUES (?1, ?2, ?3, ?4)")?;
                for object in &objects {
                    referring.execute(params![at.stream, header.epoch as i64, header.first as i64, &object.hash[..]])?;
                }
                db.prepare_cached("INSERT INTO parked_history (scope, sequence, accumulated) VALUES (?1, ?2, ?3)")?
                    .execute(params![scope_id, sequence as i64, &accumulated[..]])?;
                let signature = signer.sign(&statement("parked", &header, &[&sequence.to_le_bytes(), &accumulated]));
                return Ok(Received::Parked { sequence, accumulated, signature });
            }
            if header.epoch < at.epoch {
                return Err(invalid("a predecessor's frame is taken only up to its cutoff (§8)"));
            }
            if header.first > at.next {
                return Ok(Received::Gap { from: at.next, to: header.first - 1 });
            }
            if header.first < at.next || header.previous != at.head {
                return Err(invalid("the frame does not follow the applied prefix"));
            }
            let arriving = Arriving { frame: &frame, header: &header, records: &records };
            match apply_frame(db, scope_id, holding_id, &mut at, arriving, authorizer.as_ref())? {
                Err(why) => {
                    let signature = signer.sign(&statement("rejected", &header, &[why.as_bytes()]));
                    refuse(db, at.stream, &header, &why, &signature)?;
                    Ok(Received::Rejected { why, signature })
                }
                Ok((change, prefix)) => {
                    let signature = signer.sign(&statement("applied", &header, &[&change.to_le_bytes(), &prefix]));
                    let drained = drain(db, scope_id, holding_id, &mut at, authorizer.as_ref(), signer.as_ref())?;
                    Ok(Received::Applied { change, prefix, signature, drained })
                }
            }
        })?)
    }

    /// Moves a stream of `scope` to `epoch` once its predecessor's tail is
    /// all here (or a gap fact stands for it), applying the frames parked
    /// behind it that now follow; their first sequences.
    pub fn advance_epoch(
        &self,
        scope: &str,
        stream: &str,
        epoch: u64,
        authorizer: std::sync::Arc<dyn Authorizer>,
        signer: std::sync::Arc<dyn HubSigner>,
    ) -> Result<Vec<u64>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().write_now_at(Priority::Normal, 4096, move |db| {
            let Some((scope_id, _, Some(holding_id))) = held(db, &scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            let mut at = prefix(db, scope_id, holding_id, &stream)?;
            if epoch <= at.epoch {
                return Ok(Vec::new());
            }
            db.prepare_cached("UPDATE streams SET epoch = ?2 WHERE id = ?1")?.execute(params![at.stream, epoch as i64])?;
            at.epoch = epoch;
            drain(db, scope_id, holding_id, &mut at, authorizer.as_ref(), signer.as_ref())
        })?)
    }
}

/// Applies, in order, the parked frames of the current epoch that follow
/// the applied prefix; a refused one ends its epoch's run.
fn drain(
    db: &Connection,
    scope: i64,
    holding: i64,
    at: &mut Prefix,
    authorizer: &dyn Authorizer,
    signer: &dyn HubSigner,
) -> fastctx_storage::Result<Vec<u64>> {
    let mut drained = Vec::new();
    loop {
        let next: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = db
            .prepare_cached("SELECT header, signature, body FROM parked WHERE stream = ?1 AND epoch = ?2 AND first = ?3")?
            .query_row(params![at.stream, at.epoch as i64, at.next as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
        let Some((header_bytes, signature, body)) = next else { return Ok(drained) };
        let header = Header::decode(&header_bytes).map_err(invalid)?;
        let records = open(&header, &body).map_err(invalid)?;
        let frame = RawFrame { header: header_bytes, signature, body, objects: Vec::new() };
        for table in ["parked_objects", "parked"] {
            db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND epoch = ?2 AND first = ?3"))?
                .execute(params![at.stream, header.epoch as i64, header.first as i64])?;
        }
        if header.previous != at.head {
            let why = "it does not follow the applied prefix".to_string();
            let refusal = signer.sign(&statement("rejected", &header, &[why.as_bytes()]));
            refuse(db, at.stream, &header, &why, &refusal)?;
            return Ok(drained);
        }
        let arriving = Arriving { frame: &frame, header: &header, records: &records };
        match apply_frame(db, scope, holding, at, arriving, authorizer)? {
            Ok(_) => drained.push(header.first),
            Err(why) => {
                let refusal = signer.sign(&statement("rejected", &header, &[why.as_bytes()]));
                refuse(db, at.stream, &header, &why, &refusal)?;
                return Ok(drained);
            }
        }
    }
}
