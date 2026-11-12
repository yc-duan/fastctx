//! A stream's ordering device changing hands (§8): the epochs the hub
//! signs, the cutoff a handover sets, gap facts, and a predecessor's tail
//! found after the handover, kept as evidence.
//!
//! An epoch is one device's run of a stream. The hub signs the first when
//! it takes the stream's first frame, and each later one in a handover that
//! names the next device and where the predecessor stops: the successor's
//! first sequence and the chain value it follows. A frame is taken only
//! from its epoch's device. The predecessor's frames are taken up to the
//! cutoff; the successor's are parked until the applied prefix reaches it,
//! or until a gap fact says the rest of the predecessor's tail will never
//! come. Then the stream moves on to the successor and its parked frames
//! are applied in order. A predecessor's frame past the cutoff, or not
//! ending where the cutoff says, or come after the stream moved on, is not
//! taken: it is kept as evidence with its objects held, readable through
//! the evidence gate only where the device that brought it showed it holds
//! them (Q05).

use super::Library;
use super::exchange::RawFrame;
use super::frame::Header;
use super::hub::{Authorizer, Drained, HubSigner, Prefix, drain, prefix};
use super::scopes::held;
use crate::error::invalid;
use crate::record::ObjectRef;
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;
use std::sync::Arc;

/// What an epoch's records come from (§2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochKind {
    Source,
    Observed,
}

impl EpochKind {
    fn name(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Observed => "observed",
        }
    }
}

/// Who orders a stream next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Successor {
    pub device: [u8; 32],
    pub key: [u8; 32],
    pub kind: EpochKind,
}

/// Where a handover cuts the predecessor: the successor starts at `next`,
/// chained from `head`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cutoff {
    pub next: u64,
    pub head: [u8; 32],
}

/// A fact the hub signed, as encoded, and its signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub fact: Vec<u8>,
    pub signature: Vec<u8>,
}

/// An epoch as the hub keeps it.
pub(super) struct Epoch {
    pub epoch: u64,
    pub device: [u8; 32],
    pub key: [u8; 32],
    pub start: u64,
    pub follows: [u8; 32],
}

fn put(bytes: &mut Vec<u8>, field: &[u8]) {
    bytes.extend_from_slice(&(field.len() as u32).to_le_bytes());
    bytes.extend_from_slice(field);
}

/// An epoch's descriptor (§2.2), as signed: its stream, number, device and
/// key, first sequence, kind, and the predecessor it follows and the chain
/// value there.
#[allow(clippy::too_many_arguments)]
fn descriptor(
    scope: &str,
    stream: &str,
    epoch: u64,
    successor: &Successor,
    start: u64,
    predecessor: Option<u64>,
    follows: &[u8; 32],
) -> Vec<u8> {
    let mut bytes = b"fastctx-epoch/1".to_vec();
    put(&mut bytes, scope.as_bytes());
    put(&mut bytes, stream.as_bytes());
    bytes.extend_from_slice(&epoch.to_le_bytes());
    bytes.extend_from_slice(&successor.device);
    bytes.extend_from_slice(&successor.key);
    bytes.extend_from_slice(&start.to_le_bytes());
    put(&mut bytes, successor.kind.name().as_bytes());
    bytes.extend_from_slice(&predecessor.map_or(u64::MAX, |epoch| epoch).to_le_bytes());
    bytes.extend_from_slice(follows);
    bytes
}

/// An epoch's row as read.
struct Row {
    epoch: i64,
    device: Vec<u8>,
    key: Vec<u8>,
    start: i64,
    follows: Vec<u8>,
}

/// The epoch of `stream` the column `by` names as `number`.
fn epoch_by(db: &Connection, by: &str, stream: i64, number: u64) -> fastctx_storage::Result<Option<Epoch>> {
    let row = db
        .prepare_cached(&format!("SELECT epoch, device, key, start, follows FROM hub_epochs WHERE stream = ?1 AND {by} = ?2"))?
        .query_row(params![stream, number as i64], |row| {
            Ok(Row { epoch: row.get(0)?, device: row.get(1)?, key: row.get(2)?, start: row.get(3)?, follows: row.get(4)? })
        })
        .optional()?;
    let Some(Row { epoch, device, key, start, follows }) = row else { return Ok(None) };
    let array = |bytes: Vec<u8>| -> fastctx_storage::Result<[u8; 32]> { bytes.try_into().map_err(|_| invalid("a malformed epoch")) };
    Ok(Some(Epoch { epoch: epoch as u64, device: array(device)?, key: array(key)?, start: start as u64, follows: array(follows)? }))
}

/// The epoch `epoch` of `stream`, if the hub signed it.
pub(super) fn epoch_of(db: &Connection, stream: i64, epoch: u64) -> fastctx_storage::Result<Option<Epoch>> {
    epoch_by(db, "epoch", stream, epoch)
}

/// The epoch that follows `epoch` of `stream`, once a handover named it.
pub(super) fn successor_of(db: &Connection, stream: i64, epoch: u64) -> fastctx_storage::Result<Option<Epoch>> {
    epoch_by(db, "predecessor", stream, epoch)
}

/// Whether `header` may be taken from its device: the epoch it names is
/// the hub's and that device's, and the frame starts within it. `Ok(true)`
/// when no epoch of the stream is signed yet and the frame is of the
/// first: its device is made the first epoch's (§2.2).
pub(super) fn sequencer(db: &Connection, stream: Option<i64>, header: &Header) -> fastctx_storage::Result<bool> {
    let known = match stream {
        Some(stream) => epoch_of(db, stream, header.epoch)?,
        None => None,
    };
    match known {
        Some(epoch) if epoch.device != header.device || epoch.key != header.key => {
            Err(invalid("the frame is not of its epoch's device"))
        }
        Some(epoch) if header.first < epoch.start => Err(invalid("the frame starts before its epoch")),
        Some(_) => Ok(false),
        None => {
            let any = match stream {
                Some(stream) => db
                    .prepare_cached("SELECT 1 FROM hub_epochs WHERE stream = ?1 LIMIT 1")?
                    .query_row([stream], |_| Ok(()))
                    .optional()?
                    .is_some(),
                None => false,
            };
            if !any && header.epoch == 0 {
                Ok(true)
            } else {
                Err(invalid("the frame is of an epoch the hub has not signed"))
            }
        }
    }
}

/// Makes the stream's first epoch that of the device whose frame came
/// first.
pub(super) fn first_epoch(db: &Connection, stream: i64, header: &Header, signer: &dyn HubSigner) -> fastctx_storage::Result<()> {
    let device = Successor { device: header.device, key: header.key, kind: EpochKind::Source };
    sign_first(db, (stream, &header.scope, &header.stream), &device, signer)?;
    Ok(())
}

/// Signs and keeps the first epoch of a stream, for `device`.
fn sign_first(db: &Connection, stream: (i64, &str, &str), device: &Successor, signer: &dyn HubSigner) -> fastctx_storage::Result<Signed> {
    let (id, scope, name) = stream;
    let fact = descriptor(scope, name, 0, device, 0, None, &[0; 32]);
    let signature = signer.sign(&fact);
    db.prepare_cached(
        "INSERT INTO hub_epochs (stream, epoch, device, key, start, kind, predecessor, follows, fact, signature)
         VALUES (?1, 0, ?2, ?3, 0, ?4, NULL, ?5, ?6, ?7)",
    )?
    .execute(params![id, &device.device[..], &device.key[..], device.kind.name(), &[0u8; 32][..], fact, signature])?;
    Ok(Signed { fact, signature })
}

/// Why a frame of the stream `at` stands on is kept as evidence instead
/// of taken, if it is.
pub(super) fn evidence(db: &Connection, at: &Prefix, header: &Header) -> fastctx_storage::Result<Option<&'static str>> {
    if header.epoch < at.epoch {
        return Ok(Some("it came after its stream moved on to a later epoch"));
    }
    if header.epoch == at.epoch
        && let Some(next) = successor_of(db, at.stream, at.epoch)?
    {
        let end = header.first + u64::from(header.count);
        if end > next.start {
            return Ok(Some("it goes past where a handover cut its epoch"));
        }
        if end == next.start && header.chain() != next.follows {
            return Ok(Some("it does not end where a handover cut its epoch"));
        }
    }
    Ok(None)
}

fn holder(stream: i64, header: &Header) -> String {
    let digest: String = header.digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{stream}:{}:{}:{digest}", header.epoch, header.first)
}

/// Keeps a frame as evidence with its objects held; those in `shown` the
/// device that brought it showed it holds (§6.5's rule), and only those
/// are read through the evidence gate. The same frame again only adds to
/// what was shown.
pub(super) fn keep_evidence(
    db: &Connection,
    at: (i64, i64),
    header: &Header,
    frame: &RawFrame,
    why: &str,
    objects: &[ObjectRef],
    shown: &BTreeSet<[u8; 32]>,
) -> fastctx_storage::Result<()> {
    let (stream, holding) = at;
    let key = holder(stream, header);
    db.prepare_cached(
        "INSERT OR IGNORE INTO evidence (stream, epoch, first, digest, holding, holder, header, signature, body, why, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?
    .execute(params![
        stream,
        header.epoch as i64,
        header.first as i64,
        &header.digest[..],
        holding,
        key,
        frame.header,
        frame.signature,
        frame.body,
        why,
        super::objects::now()
    ])?;
    let mut holding_object = db.prepare_cached(
        "INSERT INTO object_holds (kind, holder, hash, proven) VALUES ('evidence', ?1, ?2, ?3)
         ON CONFLICT (kind, holder, hash) DO UPDATE SET proven = MAX(proven, excluded.proven)",
    )?;
    for object in objects {
        holding_object.execute(params![key, &object.hash[..], i64::from(shown.contains(&object.hash))])?;
    }
    Ok(())
}

/// Moves the stream on to each next epoch whose cutoff the applied prefix
/// has reached.
pub(super) fn advance(db: &Connection, at: &mut Prefix) -> fastctx_storage::Result<()> {
    while let Some(next) = successor_of(db, at.stream, at.epoch)? {
        if next.start != at.next || next.follows != at.head {
            break;
        }
        db.prepare_cached("UPDATE streams SET epoch = ?2 WHERE id = ?1")?.execute(params![at.stream, next.epoch as i64])?;
        at.epoch = next.epoch;
    }
    Ok(())
}

/// A gap fact (§8), as signed.
fn gap_fact(header: (&str, &str, u64), from: u64, to: u64, start: &[u8; 32], follows: &[u8; 32], why: &str) -> Vec<u8> {
    let (scope, stream, epoch) = header;
    let mut bytes = b"fastctx-gap/1".to_vec();
    put(&mut bytes, scope.as_bytes());
    put(&mut bytes, stream.as_bytes());
    bytes.extend_from_slice(&epoch.to_le_bytes());
    bytes.extend_from_slice(&from.to_le_bytes());
    bytes.extend_from_slice(&to.to_le_bytes());
    bytes.extend_from_slice(start);
    bytes.extend_from_slice(follows);
    put(&mut bytes, why.as_bytes());
    bytes
}

impl Library {
    /// Signs the first epoch of `stream` of `scope` for `device` before the
    /// hub took anything of it: a stream begun where the hub was not (a
    /// provisional scope's, §2.1, §2.2), or handed over before anything of
    /// it came (§8). The same device again gets the same descriptor.
    pub fn open_epoch(&self, scope: &str, stream: &str, device: Successor, signer: Arc<dyn HubSigner>) -> Result<Signed> {
        let (key, name) = (scope.to_string(), stream.to_string());
        Ok(self.database().write_now_at(Priority::Normal, 1024, move |db| {
            let Some((scope_id, state, Some(holding))) = held(db, &key)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" {
                return Err(invalid("the scope is not live"));
            }
            let at = prefix(db, scope_id, holding, &name)?;
            match epoch_of(db, at.stream, 0)? {
                Some(first) if first.device == device.device && first.key == device.key => Ok(db
                    .prepare_cached("SELECT fact, signature FROM hub_epochs WHERE stream = ?1 AND epoch = 0")?
                    .query_row([at.stream], |row| Ok(Signed { fact: row.get(0)?, signature: row.get(1)? }))?),
                Some(_) => Err(invalid("the stream's first epoch is another device's")),
                None => sign_first(db, (at.stream, &key, &name), &device, signer.as_ref()),
            }
        })?)
    }

    /// Hands `stream` of `scope` to `successor`, cutting its last epoch at
    /// `cutoff` (§8): the new epoch's signed descriptor, and the parked
    /// frames that could apply at once. The caller settled the cutoff: the
    /// furthest head signed by the predecessor that anyone could bring.
    pub fn handover(
        &self,
        scope: &str,
        stream: &str,
        successor: Successor,
        cutoff: Cutoff,
        authorizer: Arc<dyn Authorizer>,
        signer: Arc<dyn HubSigner>,
    ) -> Result<(Signed, Drained)> {
        let (key, name) = (scope.to_string(), stream.to_string());
        let (authorizing, signing) = (authorizer.clone(), signer.clone());
        let ((signed, mut drained), more) = self.database().write_now_at(Priority::Normal, 1 << 20, move |db| {
            let Some((scope_id, state, Some(holding))) = held(db, &key)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" {
                return Err(invalid("the scope is not live"));
            }
            let mut at = prefix(db, scope_id, holding, &name)?;
            let last: Option<i64> = db
                .prepare_cached("SELECT MAX(epoch) FROM hub_epochs WHERE stream = ?1")?
                .query_row([at.stream], |row| row.get(0))?;
            let Some(last) = last else { return Err(invalid("the stream has no epoch to hand over")) };
            let last = epoch_of(db, at.stream, last as u64)?.ok_or_else(|| invalid("a malformed epoch"))?;
            if cutoff.next < last.start {
                return Err(invalid("the cutoff falls before its epoch starts"));
            }
            if at.epoch == last.epoch && cutoff.next < at.next {
                return Err(invalid("the cutoff falls inside what is applied"));
            }
            if at.epoch == last.epoch && cutoff.next == at.next && cutoff.head != at.head {
                return Err(invalid("the cutoff does not follow what is applied"));
            }
            let epoch = last.epoch + 1;
            let fact = descriptor(&key, &name, epoch, &successor, cutoff.next, Some(last.epoch), &cutoff.head);
            let signature = signer.sign(&fact);
            db.prepare_cached(
                "INSERT INTO hub_epochs (stream, epoch, device, key, start, kind, predecessor, follows, fact, signature)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?
            .execute(params![
                at.stream,
                epoch as i64,
                &successor.device[..],
                &successor.key[..],
                cutoff.next as i64,
                successor.kind.name(),
                last.epoch as i64,
                &cutoff.head[..],
                fact,
                signature
            ])?;
            // The predecessor's parked frames past the cut are evidence now;
            // their objects were shown when they were parked.
            let parked: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> = {
                let mut statement = db.prepare_cached(
                    "SELECT header, signature, body FROM parked WHERE stream = ?1 AND epoch = ?2 AND holding = ?3 ORDER BY first",
                )?;
                let rows = statement.query_map(params![at.stream, last.epoch as i64, holding], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            for (header_bytes, frame_signature, body) in parked {
                let header = Header::decode(&header_bytes).map_err(invalid)?;
                let end = header.first + u64::from(header.count);
                if end < cutoff.next || (end == cutoff.next && header.chain() == cutoff.head) {
                    continue;
                }
                let why = if end > cutoff.next {
                    "it goes past where a handover cut its epoch"
                } else {
                    "it does not end where a handover cut its epoch"
                };
                let objects: Vec<ObjectRef> = {
                    let records = super::frame::open(&header, &body).map_err(invalid)?;
                    let mut seen = BTreeSet::new();
                    records
                        .iter()
                        .filter_map(|record| match &record.record.state {
                            crate::record::State::Is { objects, .. } => Some(objects.clone()),
                            crate::record::State::Gone => None,
                        })
                        .flatten()
                        .filter(|object| seen.insert(object.hash))
                        .collect()
                };
                let shown: BTreeSet<[u8; 32]> = objects.iter().map(|object| object.hash).collect();
                let frame = RawFrame { header: header_bytes, signature: frame_signature, body, objects: Vec::new() };
                keep_evidence(db, (at.stream, holding), &header, &frame, why, &objects, &shown)?;
                let place = params![at.stream, header.epoch as i64, header.first as i64, holding];
                for table in ["parked_objects", "parked"] {
                    db.prepare_cached(&format!("DELETE FROM {table} WHERE stream = ?1 AND epoch = ?2 AND first = ?3 AND holding = ?4"))?
                        .execute(place)?;
                }
            }
            advance(db, &mut at)?;
            let mut drained = Drained::default();
            let more = drain(db, scope_id, holding, &mut at, authorizer.as_ref(), signer.as_ref(), &mut drained)?;
            Ok(((Signed { fact, signature }, drained), more))
        })?;
        if more {
            self.drain_on(scope, stream, &authorizing, &signing, &mut drained)?;
        }
        Ok((signed, drained))
    }

    /// The predecessor's tail from the applied prefix to the cutoff will
    /// never come (§8): signs a gap fact over it, moves the stream on and
    /// applies what was parked behind it.
    pub fn gap(
        &self,
        scope: &str,
        stream: &str,
        why: &str,
        authorizer: Arc<dyn Authorizer>,
        signer: Arc<dyn HubSigner>,
    ) -> Result<(Signed, Drained)> {
        let (key, name, why) = (scope.to_string(), stream.to_string(), why.to_string());
        let (authorizing, signing) = (authorizer.clone(), signer.clone());
        let ((signed, mut drained), more) = self.database().write_now_at(Priority::Normal, 1 << 20, move |db| {
            let Some((scope_id, state, Some(holding))) = held(db, &key)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" {
                return Err(invalid("the scope is not live"));
            }
            let mut at = prefix(db, scope_id, holding, &name)?;
            let Some(next) = successor_of(db, at.stream, at.epoch)? else {
                return Err(invalid("no handover cut the stream's epoch"));
            };
            if at.next >= next.start {
                return Err(invalid("nothing is missing before the cutoff"));
            }
            let (from, to) = (at.next, next.start - 1);
            let fact = gap_fact((&key, &name, at.epoch), from, to, &at.head, &next.follows, &why);
            let signature = signer.sign(&fact);
            db.prepare_cached(
                "INSERT INTO gaps (stream, epoch, from_sequence, to_sequence, fact, signature) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![at.stream, at.epoch as i64, from as i64, to as i64, fact, signature])?;
            db.prepare_cached(
                "INSERT INTO cursors (stream, holding, sequence, head) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (stream, holding) DO UPDATE SET sequence = excluded.sequence, head = excluded.head",
            )?
            .execute(params![at.stream, holding, to as i64, &next.follows[..]])?;
            db.prepare_cached("UPDATE streams SET next = ?2, head = ?3, complete_through = ?4 WHERE id = ?1")?
                .execute(params![at.stream, next.start as i64, &next.follows[..], to as i64])?;
            at.next = next.start;
            at.head = next.follows;
            advance(db, &mut at)?;
            let mut drained = Drained::default();
            let more = drain(db, scope_id, holding, &mut at, authorizer.as_ref(), signer.as_ref(), &mut drained)?;
            Ok(((Signed { fact, signature }, drained), more))
        })?;
        if more {
            self.drain_on(scope, stream, &authorizing, &signing, &mut drained)?;
        }
        Ok((signed, drained))
    }

    /// The signed descriptors of `stream`'s epochs, in order, and its gap
    /// facts, for devices to check what they take (§7.5, §8).
    pub fn succession(&self, scope: &str, stream: &str) -> Result<(Vec<Signed>, Vec<Signed>)> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            let rows = |sql: &str| -> fastctx_storage::Result<Vec<Signed>> {
                let mut statement = db.prepare_cached(sql)?;
                let rows = statement.query_map(params![scope, stream], |row| Ok(Signed { fact: row.get(0)?, signature: row.get(1)? }))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            };
            Ok((
                rows(
                    "SELECT e.fact, e.signature FROM hub_epochs e JOIN streams s ON s.id = e.stream JOIN scopes c ON c.id = s.scope
                     WHERE c.key = ?1 AND s.name = ?2 ORDER BY e.epoch",
                )?,
                rows(
                    "SELECT g.fact, g.signature FROM gaps g JOIN streams s ON s.id = g.stream JOIN scopes c ON c.id = s.scope
                     WHERE c.key = ?1 AND s.name = ?2 ORDER BY g.epoch, g.from_sequence",
                )?,
            ))
        })?)
    }
}
