//! A hub's owner library taking frames (§6.5): kept, applied and published
//! as three steps.
//!
//! A frame the hub has a word on already is answered with it before
//! anything is asked of it. Any other is checked before any transaction:
//! its signature, its records, and every object it refers to present here
//! at the one length the frame gives it, with the ones its source device
//! could not read through the gate proven held by their whole bytes (P01,
//! Q01, Q02; the proof itself is the caller's exchange). Then:
//! - one that follows the stream's applied prefix is authorized record by
//!   record, by its kind's rules, against the item as the applied prefix
//!   and the frame's earlier records leave it, applied with a new change
//!   number of its scope, and answered `applied`; frames parked behind it
//!   that now follow are applied in turn;
//! - one of a later epoch, whose predecessor's tail is not all here, is
//!   kept unapplied, with the scope's parked sequence and accumulator, and
//!   answered `parked`; nothing reads it;
//! - one refused at application, and every later frame of its epoch, is
//!   answered `rejected` from then on, each frame with a signed word of its
//!   own naming where the refusal starts.
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
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Parked frames applied in one transaction: a long tail behind a handover
/// goes in several, other writes between them.
const DRAIN_AT_ONCE: usize = 16;

/// Judges whether a record may change what it changes, against the item as
/// the applied prefix has it (§11): the same rules its producer judged by.
/// The frame's header names who acts: its scope, stream, epoch and the
/// device that ordered it, whichever call brought the frame to its turn.
pub trait Authorizer: Send + Sync {
    fn authorize(&self, frame: &Header, record: &KindRecord, before: Option<&Entry>) -> std::result::Result<(), String>;

    /// Whether records of `kind` at `schema` are known here: one that is not
    /// is kept in its frame, out of the index, until it is (§2.3).
    fn knows(&self, _kind: u16, _schema: u16) -> bool {
        true
    }
}

/// Signs the hub's word on a frame with its service key (§6.1); the signer
/// binds the hub's chain head as it stands.
pub trait HubSigner: Send + Sync {
    fn sign(&self, statement: &[u8]) -> Vec<u8>;
}

/// How a frame is taken: in which of the hub's holding periods of its scope
/// (one that ended meanwhile writes nothing, §4.5), and how its source's
/// hold on the objects stands: those it may read through the gate, and
/// those it proved.
pub struct Taking<'a> {
    pub period: i64,
    pub readable: &'a (dyn Fn(&ObjectRef) -> bool + Sync),
    /// Whether the device proved it holds the object of this hash lately.
    pub proven: &'a (dyn Fn(&[u8; 32]) -> bool + Sync),
}

/// The hub's word on a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Received {
    Applied { change: u64, prefix: [u8; 32], signature: Vec<u8>, drained: Drained },
    Parked { sequence: u64, accumulated: [u8; 32], signature: Vec<u8> },
    /// Refused with every later frame of its epoch from `from`, which is
    /// the frame's own first sequence when it was the one refused.
    Rejected { why: String, from: u64, signature: Vec<u8> },
    /// Not held yet: send `from` to `to` first.
    Gap { from: u64, to: u64 },
    /// Not here yet: put these objects first.
    Missing(Vec<ObjectRef>),
    /// Prove holding these by their whole bytes first.
    Prove(Vec<ObjectRef>),
}

/// What became of the frames parked behind an applied one: how many were
/// applied after it, and the sequence the applied prefix reached with them
/// (each one's word is had again by `answer`); and the one refused, if any,
/// for its producer to hear — the parked frames after that one are its
/// producer's too, refused with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drained {
    pub applied: u64,
    pub through: Option<u64>,
    pub refused: Option<Refused>,
}

/// A parked frame refused when its turn came: its header as it came, why,
/// and the hub's signed word, refusing from its own first sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub header: Vec<u8>,
    pub why: String,
    pub signature: Vec<u8>,
}

/// What a receipt says, as signed: its kind, the frame, and where it stands.
fn statement(kind: &str, header: &Header, fields: &[&[u8]]) -> Vec<u8> {
    let frame = FrameKey { scope: &header.scope, stream: &header.stream, epoch: header.epoch, first: header.first, digest: &header.digest };
    stated(kind, &frame, fields)
}

/// A frame as a receipt names it.
pub struct FrameKey<'a> {
    pub scope: &'a str,
    pub stream: &'a str,
    pub epoch: u64,
    pub first: u64,
    pub digest: &'a [u8; 32],
}

fn stated(kind: &str, frame: &FrameKey<'_>, fields: &[&[u8]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for part in [kind.as_bytes(), frame.scope.as_bytes(), frame.stream.as_bytes()] {
        bytes.extend_from_slice(&(part.len() as u32).to_le_bytes());
        bytes.extend_from_slice(part);
    }
    bytes.extend_from_slice(&frame.epoch.to_le_bytes());
    bytes.extend_from_slice(&frame.first.to_le_bytes());
    bytes.extend_from_slice(frame.digest);
    for field in fields {
        bytes.extend_from_slice(&(field.len() as u32).to_le_bytes());
        bytes.extend_from_slice(field);
    }
    bytes
}

/// What an applied receipt on `frame` says, as the hub signed it: for
/// checking a receipt a device reports (§17).
pub fn applied_statement(frame: &FrameKey<'_>, change: u64, prefix: &[u8; 32]) -> Vec<u8> {
    stated("applied", frame, &[&change.to_le_bytes(), prefix])
}

/// What a parked receipt on `frame` says, as the hub signed it.
pub fn parked_statement(frame: &FrameKey<'_>, sequence: u64, accumulated: &[u8; 32]) -> Vec<u8> {
    stated("parked", frame, &[&sequence.to_le_bytes(), accumulated])
}

/// The hub's signed refusal of the frame `header`: why, and from which
/// sequence of its epoch frames are refused.
fn refusal(header: &Header, why: &str, from: u64, signer: &dyn HubSigner) -> Received {
    let signature = signer.sign(&statement("rejected", header, &[why.as_bytes(), &from.to_le_bytes()]));
    Received::Rejected { why: why.to_string(), from, signature }
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

/// A frame gives each object it refers to one length (I3): only the first
/// reference of a hash is checked against the object here.
fn lengths_agree(records: &[KindRecord]) -> Result<()> {
    let mut lengths = BTreeMap::new();
    for record in records {
        if let State::Is { objects, .. } = &record.record.state {
            for object in objects {
                if *lengths.entry(object.hash).or_insert(object.size) != object.size {
                    return Err(Error::Invalid("the frame refers to one object at two lengths".into()));
                }
            }
        }
    }
    Ok(())
}

/// An item's row as read, from the index or from the prefix state a floor
/// hides.
struct Seen {
    revision: i64,
    place: Vec<u8>,
    state: i64,
    kind: u16,
    schema: u16,
    body: Option<Vec<u8>>,
    objects: Option<Vec<u8>>,
}

fn seen(db: &Connection, table: &str, stream: i64, holding: i64, item: &[u8]) -> fastctx_storage::Result<Option<Seen>> {
    Ok(db
        .prepare_cached(&format!(
            "SELECT revision, place, state, kind, schema, body, objects FROM {table} WHERE stream = ?1 AND item = ?2 AND holding = ?3"
        ))?
        .query_row(params![stream, item, holding], |row| {
            Ok(Seen {
                revision: row.get(0)?,
                place: row.get(1)?,
                state: row.get(2)?,
                kind: row.get(3)?,
                schema: row.get(4)?,
                body: row.get(5)?,
                objects: row.get(6)?,
            })
        })
        .optional()?)
}

/// Whether `item` of `stream` has a record of the period `holding` kept out
/// of the index.
fn pending_item(db: &Connection, stream: i64, holding: i64, item: &[u8]) -> fastctx_storage::Result<bool> {
    Ok(db
        .prepare_cached("SELECT 1 FROM pending_items WHERE stream = ?1 AND holding = ?2 AND item = ?3")?
        .query_row(params![stream, holding, item], |_| Ok(()))
        .optional()?
        .is_some())
}

/// The item as the applied prefix of this period has it: its row in the
/// index, or, where a revision floor of an earlier period still hides it,
/// the state the prefix gave it (`prefix_items`), whichever is later.
pub(super) fn before(db: &Connection, stream: i64, holding: i64, item: &[u8]) -> fastctx_storage::Result<Option<Entry>> {
    let indexed = seen(db, "items", stream, holding, item)?;
    let hidden = seen(db, "prefix_items", stream, holding, item)?;
    let latest = match (indexed, hidden) {
        (Some(indexed), Some(hidden)) => Some(if hidden.revision > indexed.revision { hidden } else { indexed }),
        (indexed, hidden) => indexed.or(hidden),
    };
    let Some(latest) = latest else { return Ok(None) };
    entry(item, latest)
}

fn entry(item: &[u8], seen: Seen) -> fastctx_storage::Result<Option<Entry>> {
    let Seen { revision, place, state, kind, schema, body, objects } = seen;
    if state != IS {
        return Ok(None);
    }
    Ok(Some(Entry {
        item: item.to_vec(),
        order: place,
        revision: revision as u64,
        kind,
        schema,
        body: body.unwrap_or_default(),
        objects: unpack(&objects.unwrap_or_default()).map_err(invalid)?,
    }))
}

/// The item as `before` has it, where the latest revision this period's
/// index or prefix state holds of it is earlier than `revision` (or there
/// is none); `None` where it is not.
pub(super) fn before_below(
    db: &Connection,
    stream: i64,
    holding: i64,
    item: &[u8],
    revision: u64,
) -> fastctx_storage::Result<Option<Option<Entry>>> {
    let indexed = seen(db, "items", stream, holding, item)?;
    let hidden = seen(db, "prefix_items", stream, holding, item)?;
    let latest = match (indexed, hidden) {
        (Some(indexed), Some(hidden)) => Some(if hidden.revision > indexed.revision { hidden } else { indexed }),
        (indexed, hidden) => indexed.or(hidden),
    };
    match latest {
        None => Ok(Some(None)),
        Some(latest) if (latest.revision as u64) < revision => Ok(Some(entry(item, latest)?)),
        Some(_) => Ok(None),
    }
}

/// The state an earlier period's prefix gave the item, where it is earlier
/// than `revision`.
pub(super) fn hidden_before(db: &Connection, stream: i64, holding: i64, item: &[u8], revision: u64) -> fastctx_storage::Result<Option<Entry>> {
    match seen(db, "prefix_items", stream, holding, item)? {
        Some(hidden) if (hidden.revision as u64) < revision => entry(item, hidden),
        _ => Ok(None),
    }
}

/// A record the index will not take because a later revision of an earlier
/// period stands there — its floor, or its row not yet purged — is kept as
/// the state this period's prefix gives its item, for what is judged
/// against it later.
pub(super) fn hidden_by_floor(db: &Connection, at: Applying, revision: u64, record: &KindRecord) -> fastctx_storage::Result<()> {
    let earlier: Option<i64> = db
        .prepare_cached("SELECT revision FROM items WHERE stream = ?1 AND item = ?2 AND (holding != ?3 OR state = ?4)")?
        .query_row(params![at.stream, record.record.item, at.holding, super::items::FLOOR], |row| row.get(0))
        .optional()?;
    if earlier.is_none_or(|earlier| earlier <= revision as i64) {
        return Ok(());
    }
    let (state, body, objects) = match &record.record.state {
        State::Is { body, objects } => (IS, Some(body.clone()), Some(crate::record::pack(objects))),
        State::Gone => (super::items::GONE, None, None),
    };
    db.prepare_cached(
        "INSERT INTO prefix_items (stream, item, holding, revision, place, state, kind, schema, body, objects)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (stream, item, holding) DO UPDATE SET revision = excluded.revision, place = excluded.place,
            state = excluded.state, kind = excluded.kind, schema = excluded.schema, body = excluded.body, objects = excluded.objects
         WHERE excluded.revision > prefix_items.revision",
    )?
    .execute(params![
        at.stream,
        record.record.item,
        at.holding,
        revision as i64,
        record.record.order,
        state,
        record.kind,
        record.schema,
        body,
        objects
    ])?;
    Ok(())
}

/// The item as `record`, at `revision`, leaves it.
pub(super) fn after(record: &KindRecord, revision: u64) -> Option<Entry> {
    match &record.record.state {
        State::Is { body, objects } => Some(Entry {
            item: record.record.item.clone(),
            order: record.record.order.clone(),
            revision,
            kind: record.kind,
            schema: record.schema,
            body: body.clone(),
            objects: objects.clone(),
        }),
        State::Gone => None,
    }
}

/// The applied prefix of a stream: its id, epoch, next sequence and chain
/// value.
pub(super) struct Prefix {
    pub stream: i64,
    pub epoch: u64,
    pub next: u64,
    pub head: [u8; 32],
}

/// The applied prefix of a stream, made at need.
pub(super) fn prefix(db: &Connection, scope: i64, holding: i64, name: &str) -> fastctx_storage::Result<Prefix> {
    if let Some(at) = standing(db, scope, holding, name)? {
        return Ok(at);
    }
    db.prepare_cached("INSERT INTO streams (scope, name, epoch, next, head, complete_through) VALUES (?1, ?2, 0, 0, NULL, NULL)")?
        .execute(params![scope, name])?;
    Ok(Prefix { stream: db.last_insert_rowid(), epoch: 0, next: 0, head: [0; 32] })
}

/// The applied prefix of a stream already here.
fn standing(db: &Connection, scope: i64, holding: i64, name: &str) -> fastctx_storage::Result<Option<Prefix>> {
    let found: Option<(i64, i64)> = db
        .prepare_cached("SELECT id, epoch FROM streams WHERE scope = ?1 AND name = ?2")?
        .query_row(params![scope, name], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((stream, epoch)) = found else { return Ok(None) };
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
    Ok(Some(Prefix { stream, epoch: epoch as u64, next, head }))
}

/// The hub's word on a frame it has one on already (§6.5 steps 1 and 3, a
/// lost receipt): its refusal, or that of an earlier frame of its epoch,
/// which the refused frame's own word does not stand for; its application;
/// its being parked.
fn known(
    db: &Connection,
    scope: i64,
    holding: i64,
    header: &Header,
    bytes: &[u8],
    signer: &dyn HubSigner,
) -> fastctx_storage::Result<Option<Received>> {
    let Some(stream) = db
        .prepare_cached("SELECT id FROM streams WHERE scope = ?1 AND name = ?2")?
        .query_row(params![scope, header.stream], |row| row.get::<_, i64>(0))
        .optional()?
    else {
        return Ok(None);
    };
    let (epoch, first) = (header.epoch as i64, header.first as i64);
    let refused: Option<(i64, Vec<u8>, String, Vec<u8>)> = db
        .prepare_cached(
            "SELECT first, digest, why, signature FROM refused_frames WHERE stream = ?1 AND epoch = ?2 AND first <= ?3
             ORDER BY first DESC LIMIT 1",
        )?
        .query_row(params![stream, epoch, first], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .optional()?;
    if let Some((from, digest, why, signature)) = refused {
        if from == first && digest == header.digest {
            return Ok(Some(Received::Rejected { why, from: from as u64, signature }));
        }
        return Ok(Some(refusal(header, &why, from as u64, signer)));
    }
    let applied: Option<(Vec<u8>, Option<i64>)> = db
        .prepare_cached("SELECT header, change FROM frames WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
        .query_row(params![stream, epoch, first, holding], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    if let Some((kept, change)) = applied {
        if kept != bytes {
            return Err(invalid("another frame is applied at that position"));
        }
        let change = change.unwrap_or_default() as u64;
        let prefix: Vec<u8> = db
            .prepare_cached("SELECT prefix FROM change_log WHERE scope = ?1 AND change = ?2")?
            .query_row(params![scope, change as i64], |row| row.get(0))?;
        let prefix: [u8; 32] = prefix.try_into().map_err(|_| invalid("a malformed prefix"))?;
        let signature = signer.sign(&statement("applied", header, &[&change.to_le_bytes(), &prefix]));
        return Ok(Some(Received::Applied { change, prefix, signature, drained: Drained::default() }));
    }
    let parked: Option<(Vec<u8>, i64)> = db
        .prepare_cached("SELECT header, sequence FROM parked WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
        .query_row(params![stream, epoch, first, holding], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    if let Some((kept, sequence)) = parked {
        if kept != bytes {
            return Err(invalid("another frame is parked at that position"));
        }
        let accumulated: Vec<u8> = db
            .prepare_cached("SELECT accumulated FROM parked_history WHERE scope = ?1 AND sequence = ?2")?
            .query_row(params![scope, sequence], |row| row.get(0))?;
        let accumulated: [u8; 32] = accumulated.try_into().map_err(|_| invalid("a malformed accumulator"))?;
        let signature = signer.sign(&statement("parked", header, &[&(sequence as u64).to_le_bytes(), &accumulated]));
        return Ok(Some(Received::Parked { sequence: sequence as u64, accumulated, signature }));
    }
    Ok(None)
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
    // A record is judged against its item as the records before it in the
    // frame leave it: they come in order, each a revision of its own. One
    // whose item has an earlier record out of the index — of a kind not
    // known here, in this frame or before it — is not judged: it waits for
    // that one (V01).
    let mut earlier: BTreeMap<&[u8], Option<Entry>> = BTreeMap::new();
    let mut out: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
    let mut waiting: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (offset, record) in records.iter().enumerate() {
        let item = record.record.item.as_slice();
        if !authorizer.knows(record.kind, record.schema) {
            out.insert(item);
            continue;
        }
        if out.contains(item) || pending_item(db, at.stream, holding, item)? {
            out.insert(item);
            waiting.insert(offset);
            continue;
        }
        let prior = match earlier.remove(item) {
            Some(prior) => prior,
            None => before(db, at.stream, holding, item)?,
        };
        if let Err(why) = authorizer.authorize(header, record, prior.as_ref()) {
            return Ok(Err(why));
        }
        earlier.insert(item, after(record, header.first + offset as u64));
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
        if !authorizer.knows(record.kind, record.schema) || waiting.contains(&offset) {
            // Kept in its frame, out of the index, until this hub knows its
            // kind (§2.3), or the earlier record of its item it waits for.
            if waiting.contains(&offset) {
                db.prepare_cached("INSERT OR IGNORE INTO deferred_records (stream, epoch, first, head, offset) VALUES (?1, ?2, ?3, ?4, ?5)")?
                    .execute(params![at.stream, header.epoch as i64, header.first as i64, &head[..], offset as i64])?;
            } else {
                super::pending::keep(db, at.stream, record.kind, record.schema, header.epoch, header.first, &head)?;
            }
            db.prepare_cached("INSERT OR IGNORE INTO pending_items (stream, holding, item) VALUES (?1, ?2, ?3)")?
                .execute(params![at.stream, holding, &record.record.item])?;
            continue;
        }
        hidden_by_floor(db, applying, header.first + offset as u64, record)?;
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
    // What devices ask after where they stand moves with it (§28.1).
    let bytes = frame.header.len() + frame.signature.len() + frame.body.len();
    super::changes::moved(db, at.stream, bytes as u64, false)?;
    at.next = end;
    at.head = head;
    super::handover::advance(db, at)?;
    Ok(Ok((change, prefix)))
}

/// Refuses a frame and every later frame of its epoch, parked or still to
/// come; the hub's signature on the refusal. The objects of the parked ones
/// stay held, unreadable, while the period lasts (§6.4).
fn refuse(
    db: &Connection,
    stream: i64,
    holding: i64,
    header: &Header,
    why: &str,
    signer: &dyn HubSigner,
) -> fastctx_storage::Result<Vec<u8>> {
    let signature = signer.sign(&statement("rejected", header, &[why.as_bytes(), &header.first.to_le_bytes()]));
    db.prepare_cached(
        "INSERT OR REPLACE INTO refused_frames (stream, epoch, first, digest, why, signature) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?
    .execute(params![stream, header.epoch as i64, header.first as i64, &header.digest[..], why, &signature])?;
    let at = params![stream, header.epoch as i64, header.first as i64, holding];
    db.prepare_cached(
        "INSERT OR IGNORE INTO refused_objects (stream, epoch, first, holding, hash)
         SELECT stream, epoch, first, holding, hash FROM parked_objects WHERE stream = ?1 AND epoch = ?2 AND first >= ?3 AND holding = ?4",
    )?
    .execute(at)?;
    for table in ["parked_objects", "parked"] {
        db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND epoch = ?2 AND first >= ?3 AND holding = ?4"))?
            .execute(at)?;
    }
    Ok(signature)
}

impl Library {
    /// Takes one frame of `scope` as a hub.
    pub fn take(
        &self,
        scope: &str,
        frame: RawFrame,
        verifier: &dyn Verifier,
        taking: &Taking<'_>,
        authorizer: Arc<dyn Authorizer>,
        signer: Arc<dyn HubSigner>,
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
        lengths_agree(&records)?;
        // A word given already is given again before any object is asked
        // for: a refused frame's are not wanted, nor a known one's again.
        // Nor are those of a frame kept only as evidence: it is kept with
        // what its device showed it holds (§8).
        // A parked frame that follows the applied prefix now (a drain cut
        // short) goes on to be applied; its objects were shown when it was
        // parked. Neither is asked for objects.
        let (given, unasked) = self.database().read_now(|db| match held(db, scope)? {
            Some((scope_id, state, Some(holding))) if state == "live" && holding == taking.period => {
                let at = standing(db, scope_id, holding, &header.stream)?;
                super::handover::sequencer(db, at.as_ref().map(|at| at.stream), &header)?;
                if let Some(at) = &at
                    && follows_parked(db, at, holding, &header)?
                {
                    return Ok((None, true));
                }
                let given = known(db, scope_id, holding, &header, &frame.header, signer.as_ref())?;
                let evidence = match &at {
                    Some(at) if given.is_none() => super::handover::evidence(db, at, &header)?.is_some(),
                    _ => false,
                };
                Ok((given, evidence))
            }
            _ => Ok((None, false)),
        })?;
        if let Some(given) = given {
            return Ok(given);
        }
        let objects = objects_of(&records);
        let (mut missing, mut unproven, mut shown) = (Vec::new(), Vec::new(), BTreeSet::new());
        for object in &objects {
            match self.size(&object.hash)? {
                Some(size) if size == object.size => {
                    if (taking.readable)(object) || (taking.proven)(&object.hash) {
                        shown.insert(object.hash);
                    } else {
                        unproven.push(*object);
                    }
                }
                Some(_) => return Err(Error::Invalid("an object of that hash is here with another length".into())),
                None => missing.push(*object),
            }
        }
        if !unasked && !missing.is_empty() {
            return Ok(Received::Missing(missing));
        }
        if !unasked && !unproven.is_empty() {
            return Ok(Received::Prove(unproven));
        }
        let (scope, period) = (scope.to_string(), taking.period);
        let (key, stream) = (scope.clone(), header.stream.clone());
        let holding = period;
        let (authorizing, signing) = (authorizer.clone(), signer.clone());
        let bytes = frame.body.len() + frame.header.len() + 1024 + records.len() * 64;
        let (mut received, more) = self.database().write_now_at(Priority::Normal, bytes, move |db| {
            let Some((scope_id, state, Some(holding_id))) = held(db, &scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" {
                return Err(invalid("the scope is not live"));
            }
            if holding_id != period {
                return Err(invalid("the holding period the frame was taken in has ended"));
            }
            let mut at = prefix(db, scope_id, holding_id, &header.stream)?;
            if super::handover::sequencer(db, Some(at.stream), &header)? {
                super::handover::first_epoch(db, at.stream, &header, signer.as_ref())?;
            }
            // A parked frame sent again that follows the applied prefix now
            // (a drain cut short) is applied, with what follows it.
            let (mut resumed, mut more) = (Drained::default(), false);
            if follows_parked(db, &at, holding_id, &header)? {
                more = drain(db, scope_id, holding_id, &mut at, authorizer.as_ref(), signer.as_ref(), &mut resumed)?;
            }
            if let Some(mut given) = known(db, scope_id, holding_id, &header, &frame.header, signer.as_ref())? {
                if let Received::Applied { drained, .. } = &mut given {
                    *drained = resumed;
                }
                return Ok((given, more));
            }
            if let Some(why) = super::handover::evidence(db, &at, &header)? {
                super::handover::keep_evidence(db, (at.stream, holding_id), &header, &frame, why, &objects, &shown)?;
                return Ok((refusal(&header, why, header.first, signer.as_ref()), false));
            }
            for object in &objects {
                if present(db, &object.hash)?.is_none() {
                    return Err(invalid("an object went while the frame was checked"));
                }
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
                    "INSERT INTO parked (stream, epoch, first, holding, header, signature, body, sequence)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?
                .execute(params![
                    at.stream,
                    header.epoch as i64,
                    header.first as i64,
                    holding_id,
                    frame.header,
                    frame.signature,
                    frame.body,
                    sequence as i64
                ])?;
                let mut referring = db.prepare_cached(
                    "INSERT OR IGNORE INTO parked_objects (stream, epoch, first, holding, hash) VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                for object in &objects {
                    referring.execute(params![at.stream, header.epoch as i64, header.first as i64, holding_id, &object.hash[..]])?;
                }
                db.prepare_cached("INSERT INTO parked_history (scope, sequence, accumulated) VALUES (?1, ?2, ?3)")?
                    .execute(params![scope_id, sequence as i64, &accumulated[..]])?;
                let signature = signer.sign(&statement("parked", &header, &[&sequence.to_le_bytes(), &accumulated]));
                return Ok((Received::Parked { sequence, accumulated, signature }, false));
            }
            if header.first > at.next {
                return Ok((Received::Gap { from: at.next, to: header.first - 1 }, false));
            }
            if header.first < at.next || header.previous != at.head {
                return Err(invalid("the frame does not follow the applied prefix"));
            }
            let arriving = Arriving { frame: &frame, header: &header, records: &records };
            match apply_frame(db, scope_id, holding_id, &mut at, arriving, authorizer.as_ref())? {
                Err(why) => {
                    let signature = refuse(db, at.stream, holding_id, &header, &why, signer.as_ref())?;
                    Ok((Received::Rejected { why, from: header.first, signature }, false))
                }
                Ok((change, prefix)) => {
                    let signature = signer.sign(&statement("applied", &header, &[&change.to_le_bytes(), &prefix]));
                    let mut drained = Drained::default();
                    let more = drain(db, scope_id, holding_id, &mut at, authorizer.as_ref(), signer.as_ref(), &mut drained)?;
                    Ok((Received::Applied { change, prefix, signature, drained }, more))
                }
            }
        })?;
        if more && let Received::Applied { drained, .. } = &mut received {
            self.drain_on(&key, &stream, holding, &authorizing, &signing, drained)?;
        }
        Ok(received)
    }

    /// Goes on applying the parked frames of `stream` that follow, a few
    /// to a transaction, while the scope is held in the period `holding`
    /// the drain began in; a later period's frames are its own drain's.
    pub(super) fn drain_on(
        &self,
        scope: &str,
        stream: &str,
        holding: i64,
        authorizer: &Arc<dyn Authorizer>,
        signer: &Arc<dyn HubSigner>,
        drained: &mut Drained,
    ) -> Result<()> {
        loop {
            let (scope, stream) = (scope.to_string(), stream.to_string());
            let (authorizer, signer) = (authorizer.clone(), signer.clone());
            let (batch, more) = self.database().write_now_at(Priority::Normal, 1 << 20, move |db| {
                let mut batch = Drained::default();
                let Some((scope_id, state, Some(now))) = held(db, &scope)? else { return Ok((batch, false)) };
                if now != holding || state != "live" {
                    return Ok((batch, false));
                }
                let mut at = prefix(db, scope_id, holding, &stream)?;
                let more = drain(db, scope_id, holding, &mut at, authorizer.as_ref(), signer.as_ref(), &mut batch)?;
                Ok((batch, more))
            })?;
            drained.applied += batch.applied;
            if batch.through.is_some() {
                drained.through = batch.through;
            }
            if batch.refused.is_some() {
                drained.refused = batch.refused;
            }
            if !more {
                return Ok(());
            }
        }
    }

    /// Applies the parked frames that follow each stream's applied prefix,
    /// where a drain was cut short (§19): at a hub's opening, and from time
    /// to time. How many streams it moved.
    pub fn settle_parked(&self, authorizer: Arc<dyn Authorizer>, signer: Arc<dyn HubSigner>) -> Result<usize> {
        let waiting: Vec<(String, String, i64)> = self.database().read_now(|db| {
            let mut statement = db.prepare_cached(
                "SELECT c.key, s.name, c.holding FROM streams s JOIN scopes c ON c.id = s.scope
                 WHERE c.state = 'live' AND c.holding IS NOT NULL AND EXISTS (
                    SELECT 1 FROM parked p WHERE p.stream = s.id AND p.holding = c.holding AND p.epoch = s.epoch
                    AND p.first = COALESCE((SELECT sequence + 1 FROM cursors WHERE stream = s.id AND holding = c.holding), 0))",
            )?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })?;
        for (scope, stream, holding) in &waiting {
            let mut drained = Drained::default();
            self.drain_on(scope, stream, *holding, &authorizer, &signer, &mut drained)?;
        }
        Ok(waiting.len())
    }

    /// The hub's word on a frame of `scope` it has one on already, from the
    /// frame's header as signed (§6.5 step 1); `None` when it has none.
    pub fn answer(
        &self,
        scope: &str,
        header: &[u8],
        signature: &[u8],
        verifier: &dyn Verifier,
        signer: &dyn HubSigner,
    ) -> Result<Option<Received>> {
        let decoded = Header::decode(header).map_err(Error::Invalid)?;
        if decoded.scope != scope {
            return Err(Error::Invalid("the frame is of another scope".into()));
        }
        if !verifier.verify(&decoded, header, signature) {
            return Err(Error::Invalid("the frame's signature does not verify".into()));
        }
        Ok(self.database().read_now(|db| match held(db, scope)? {
            Some((scope_id, _, Some(holding))) => known(db, scope_id, holding, &decoded, header, signer),
            _ => Ok(None),
        })?)
    }
}

/// Applies, in order, up to `DRAIN_AT_ONCE` of the parked frames of the
/// current epoch that follow the applied prefix, moving on through the
/// epochs whose cutoffs it reaches; a refused one ends its epoch's run.
/// Whether more may follow.
pub(super) fn drain(
    db: &Connection,
    scope: i64,
    holding: i64,
    at: &mut Prefix,
    authorizer: &dyn Authorizer,
    signer: &dyn HubSigner,
    drained: &mut Drained,
) -> fastctx_storage::Result<bool> {
    for _ in 0..DRAIN_AT_ONCE {
        let next: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = db
            .prepare_cached("SELECT header, signature, body FROM parked WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
            .query_row(params![at.stream, at.epoch as i64, at.next as i64, holding], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
        let Some((header_bytes, signature, body)) = next else { return Ok(false) };
        let header = Header::decode(&header_bytes).map_err(invalid)?;
        let records = open(&header, &body).map_err(invalid)?;
        let frame = RawFrame { header: header_bytes, signature, body, objects: Vec::new() };
        let (stream, epoch, first) = (at.stream, header.epoch as i64, header.first as i64);
        let place = params![stream, epoch, first, holding];
        db.prepare_cached("DELETE FROM parked WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?.execute(place)?;
        if header.previous != at.head {
            let why = "it does not follow the applied prefix".to_string();
            let signature = refuse(db, at.stream, holding, &header, &why, signer)?;
            drained.refused = Some(Refused { header: frame.header, why, signature });
            return Ok(false);
        }
        let arriving = Arriving { frame: &frame, header: &header, records: &records };
        match apply_frame(db, scope, holding, at, arriving, authorizer)? {
            Ok(_) => {
                // Its objects are held by the applied frame now.
                db.prepare_cached("DELETE FROM parked_objects WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
                    .execute(place)?;
                drained.applied += 1;
                drained.through = Some(at.next);
            }
            Err(why) => {
                let signature = refuse(db, at.stream, holding, &header, &why, signer)?;
                drained.refused = Some(Refused { header: frame.header, why, signature });
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Whether `header` is a frame parked here that follows the applied prefix
/// of its stream now.
fn follows_parked(db: &Connection, at: &Prefix, holding: i64, header: &Header) -> fastctx_storage::Result<bool> {
    if header.epoch != at.epoch || header.first != at.next {
        return Ok(false);
    }
    Ok(db
        .prepare_cached("SELECT 1 FROM parked WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4")?
        .query_row(params![at.stream, header.epoch as i64, header.first as i64, holding], |_| Ok(()))
        .optional()?
        .is_some())
}
