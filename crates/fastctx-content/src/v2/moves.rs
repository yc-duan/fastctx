//! Who may take a stream's ordering over (design §28.10): the device that
//! orders it now, by a release it signed naming the machine it goes to, or
//! the account that opened the conversation once the device it lived on is
//! lost for good. Either is kept as the stream's move, in the transaction
//! that hands it over; asked again, the hub answers the same epoch while it
//! is the stream's last, and says it was handed on since once it is not.
//!
//! The cut is where the predecessor's frames the hub holds reach, in one
//! unbroken chain from where its epoch starts: what the hub applied of it
//! and what it keeps parked. A release reaches no further: the machine it
//! goes to first sends what of the predecessor's tail it brought, and a
//! release past what the hub holds is refused until it has. A release short
//! of it — the predecessor wrote on and sent that too — still lets the
//! machine take over, at the furthest the hub holds.
//!
//! On a device, the descriptor of the epoch it takes over in is taken and
//! the epoch opened in one transaction; one handed on since is taken as
//! history alone.

use super::Library;
use super::frame::Header;
use super::handover::{Cutoff, Epoch, Signed, Successor, cut_over, epoch_of};
use super::hub::{Authorizer, Drained, HubSigner, prefix};
use super::scopes::held;
use super::succession::{HubVerifier, known_epoch, stream_of};
use super::facts::EpochFact;
use crate::error::{Error, invalid};
use crate::Result;
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::sync::Arc;

/// A predecessor's word that the machine `to`, whose device key is
/// `to_key`, takes `stream` of `scope` over from its epoch `epoch` on the hub
/// `hub`, the predecessor having written as far as `next`, chained to
/// `head`. The key binds it: another device given the machine's name since
/// cannot use it, and the machine renamed still can (AP01, AP13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub hub: String,
    pub scope: String,
    pub stream: String,
    pub epoch: u64,
    pub to: String,
    pub to_key: [u8; 32],
    pub next: u64,
    pub head: [u8; 32],
}

impl Release {
    /// What the predecessor's device key signs.
    pub fn statement(&self) -> Vec<u8> {
        let mut bytes = b"fastctx-release/2".to_vec();
        for text in [&self.hub, &self.scope, &self.stream, &self.to] {
            bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
            bytes.extend_from_slice(text.as_bytes());
        }
        bytes.extend_from_slice(&self.to_key);
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.extend_from_slice(&self.next.to_le_bytes());
        bytes.extend_from_slice(&self.head);
        bytes
    }
}

/// What taking an epoch handed over did on a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken {
    /// The epoch is this device's and open: it writes in it.
    Opened,
    /// Handed on since: taken as history alone.
    History,
    /// The stream here ends before the epoch begins: nothing of it is known
    /// here yet, and the stream is followed as a replica's until it reaches
    /// that start (AP03).
    Behind,
    /// The stream here goes on past where the epoch, a successor's, begins:
    /// what this device wrote from there on is no frame of it, and nothing
    /// of it is opened — opened, the stream would have two histories there.
    Ahead,
    /// The stream here reaches where the epoch begins by another chain than
    /// the one the epoch follows: nothing of it is opened.
    Forked,
}

/// A stream's chain value as kept: none before its first frame.
fn chain_value(head: Option<Vec<u8>>) -> fastctx_storage::Result<[u8; 32]> {
    match head {
        None => Ok([0; 32]),
        Some(head) => head.try_into().map_err(|_| invalid("a malformed chain value")),
    }
}

/// What came of taking the hub's word on the epoch this device orders a
/// stream in (`Library::begin_epoch`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Begun {
    /// It is the epoch written in here, under the descriptor the hub signed.
    InForce,
    /// Opened here now, the stream standing where it begins.
    Opened,
    /// Not taken, and why: the stream here is not where the epoch has it.
    Refused(String),
}

/// How a signature by a device key is checked: the key, what is signed,
/// the signature.
pub type KeyCheck<'a> = &'a dyn Fn(&[u8; 32], &[u8], &[u8]) -> bool;

/// A stream this device orders, as a release of it would say it: the epoch
/// it orders it in, where it reached — the next sequence and the chain value
/// there — and where the hub's taking of it goes on from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Releasable {
    pub stream: String,
    pub epoch: u64,
    pub next: u64,
    pub head: [u8; 32],
    pub uploaded: u64,
}

/// What lets a machine take a stream over.
pub enum Authority<'a> {
    /// The predecessor's release, its signature, and how a signature by a
    /// device key is checked.
    Release { release: &'a Release, signature: &'a [u8], verify: KeyCheck<'a> },
    /// The account that opened the conversation, from the epoch `from`.
    Adopt { from: u64 },
}

/// What a move came to.
pub enum Moved {
    /// The machine orders the stream in the epoch described.
    Handed { descriptor: Signed, drained: Drained },
    /// It was handed on since: the epochs after the one it took over in.
    Superseded { epochs: Vec<Signed> },
}

/// The authority, checked where it can be before the transaction.
enum Basis {
    Release { epoch: u64, next: u64, head: [u8; 32] },
    Adopt { from: u64 },
}

impl Basis {
    fn from(&self) -> u64 {
        match self {
            Self::Release { epoch, .. } => *epoch,
            Self::Adopt { from } => *from,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Release { .. } => "release",
            Self::Adopt { .. } => "adopt",
        }
    }
}

/// Where the last epoch's frames the hub holds reach from where it starts,
/// unbroken: the next sequence and the chain value there.
fn reach(db: &Connection, at: &super::hub::Prefix, last: &Epoch, holding: i64) -> fastctx_storage::Result<(u64, [u8; 32])> {
    let mut reach = if at.epoch == last.epoch { (at.next, at.head) } else { (last.start, last.follows) };
    let headers: Vec<Vec<u8>> = {
        let mut statement =
            db.prepare_cached("SELECT header FROM parked WHERE stream = ?1 AND epoch = ?2 AND holding = ?3 ORDER BY first")?;
        let rows = statement.query_map(params![at.stream, last.epoch as i64, holding], |row| row.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for bytes in headers {
        let header = Header::decode(&bytes).map_err(invalid)?;
        let end = header.first + u64::from(header.count);
        if end <= reach.0 {
            continue;
        }
        if header.first != reach.0 || header.previous != reach.1 {
            break;
        }
        reach = (end, header.chain());
    }
    Ok(reach)
}

/// The chain value where the last epoch's frames the hub holds reach
/// `next`, if they pass through it.
fn chain_at(db: &Connection, stream: i64, last: &Epoch, holding: i64, next: u64) -> fastctx_storage::Result<Option<[u8; 32]>> {
    if next == last.start {
        return Ok(Some(last.follows));
    }
    let kept: Option<Vec<u8>> = db
        .prepare_cached("SELECT head FROM frames WHERE stream = ?1 AND holding = ?3 AND first + count - 1 = ?4 AND epoch = ?2")?
        .query_row(params![stream, last.epoch as i64, holding, next as i64 - 1], |row| row.get(0))
        .optional()?;
    if let Some(head) = kept {
        return Ok(head.try_into().ok());
    }
    let headers: Vec<Vec<u8>> = {
        let mut statement = db.prepare_cached("SELECT header FROM parked WHERE stream = ?1 AND epoch = ?2 AND holding = ?3")?;
        let rows = statement.query_map(params![stream, last.epoch as i64, holding], |row| row.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for bytes in headers {
        let header = Header::decode(&bytes).map_err(invalid)?;
        if header.first + u64::from(header.count) == next {
            return Ok(Some(header.chain()));
        }
    }
    Ok(None)
}

impl Library {
    /// Hands `stream` of `scope` over to `successor`, the machine `machine`,
    /// under `authority` (design §28.10): the move kept with the new epoch,
    /// in one transaction, or the same answer for the same move asked again.
    #[allow(clippy::too_many_arguments)]
    pub fn move_stream(
        &self,
        scope: &str,
        stream: &str,
        machine: &str,
        successor: Successor,
        authority: Authority<'_>,
        authorizer: Arc<dyn Authorizer>,
        signer: Arc<dyn HubSigner>,
    ) -> Result<Moved> {
        // A release is checked against the key of the epoch it names before
        // the transaction; the transaction holds the stream to that epoch.
        let basis = match authority {
            Authority::Release { release, signature, verify } => {
                if release.scope != scope || release.stream != stream || release.to_key != successor.key {
                    return Err(Error::Invalid("the release is of another stream or machine".into()));
                }
                let Some((last, key)) = self.last_epoch(scope, stream)? else {
                    return Err(Error::Invalid("the stream has no epoch to hand over".into()));
                };
                // Not of the last epoch, the move is found or refused below.
                if last == release.epoch && !verify(&key, &release.statement(), signature) {
                    return Err(Error::Invalid("the release is not the predecessor's".into()));
                }
                Basis::Release { epoch: release.epoch, next: release.next, head: release.head }
            }
            Authority::Adopt { from } => Basis::Adopt { from },
        };
        let (key, name, machine) = (scope.to_string(), stream.to_string(), machine.to_string());
        let (authorizing, signing) = (authorizer.clone(), signer.clone());
        let (moved, more, holding) = self.database().write_now_at(Priority::Normal, 1 << 20, move |db| {
            let Some((scope_id, state, Some(holding))) = held(db, &key)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" {
                return Err(invalid("the scope is not live"));
            }
            let mut at = prefix(db, scope_id, holding, &name)?;
            let last: Option<i64> =
                db.prepare_cached("SELECT MAX(epoch) FROM hub_epochs WHERE stream = ?1")?.query_row([at.stream], |row| row.get(0))?;
            let Some(last) = last else { return Err(invalid("the stream has no epoch to hand over")) };
            let last = epoch_of(db, at.stream, last as u64)?.ok_or_else(|| invalid("a malformed epoch"))?;
            // The same move asked again — from the same predecessor's epoch
            // to the same device — answered the same while its epoch is the
            // last, as handed on since once it is not (AN02); a later move
            // to the same device from a later epoch is another (AP02).
            let made: Option<i64> = db
                .prepare_cached("SELECT epoch FROM moves WHERE stream = ?1 AND predecessor = ?2 AND successor = ?3")?
                .query_row(params![at.stream, basis.from() as i64, &successor.key[..]], |row| row.get(0))
                .optional()?;
            if let Some(made) = made {
                let made = made as u64;
                if made == last.epoch {
                    let (fact, signature): (Vec<u8>, Vec<u8>) = db
                        .prepare_cached("SELECT fact, signature FROM hub_epochs WHERE stream = ?1 AND epoch = ?2")?
                        .query_row(params![at.stream, made as i64], |row| Ok((row.get(0)?, row.get(1)?)))?;
                    return Ok((Moved::Handed { descriptor: Signed { fact, signature }, drained: Drained::default() }, false, holding));
                }
                if made < last.epoch {
                    let mut statement =
                        db.prepare_cached("SELECT fact, signature FROM hub_epochs WHERE stream = ?1 AND epoch > ?2 ORDER BY epoch")?;
                    let rows = statement.query_map(params![at.stream, made as i64], |row| Ok(Signed { fact: row.get(0)?, signature: row.get(1)? }))?;
                    return Ok((Moved::Superseded { epochs: rows.collect::<std::result::Result<_, _>>()? }, false, holding));
                }
            }
            if basis.from() != last.epoch {
                return Err(invalid("the stream was handed over since the move was allowed"));
            }
            if last.key == successor.key {
                return Err(invalid("the machine orders the stream already"));
            }
            let (next, head) = reach(db, &at, &last, holding)?;
            if let Basis::Release { next: released, head: chained, .. } = &basis {
                if *released > next {
                    return Err(invalid("the predecessor's tail to where it released the stream is not here yet"));
                }
                if chain_at(db, at.stream, &last, holding, *released)? != Some(*chained) {
                    return Err(invalid("the release names a place the predecessor's frames do not pass through"));
                }
            }
            let (descriptor, drained, more) = cut_over(
                db,
                (&key, &name, scope_id, holding),
                &mut at,
                &last,
                &successor,
                Cutoff { next, head },
                authorizer.as_ref(),
                signer.as_ref(),
            )?;
            db.prepare_cached(
                "INSERT INTO moves (stream, predecessor, epoch, machine, successor, basis, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?
            .execute(params![
                at.stream,
                last.epoch as i64,
                (last.epoch + 1) as i64,
                machine,
                &successor.key[..],
                basis.name(),
                super::objects::now()
            ])?;
            Ok((Moved::Handed { descriptor, drained }, more, holding))
        })?;
        let moved = match moved {
            Moved::Handed { descriptor, mut drained } => {
                if more {
                    self.drain_on(scope, stream, holding, &authorizing, &signing, &mut drained)?;
                }
                Moved::Handed { descriptor, drained }
            }
            superseded => superseded,
        };
        Ok(moved)
    }

    /// Takes over ordering `stream` of `scope` here in the epoch `signed`
    /// describes, the hub's word on it checked: its descriptor taken and the
    /// epoch opened in one transaction (AG06). An epoch of the stream later
    /// than it is known here: it is taken as history, and nothing is opened
    /// (AN02). Whether the epoch was opened.
    pub fn take_over(&self, scope: &str, signed: Signed, verifier: &dyn HubVerifier) -> Result<Taken> {
        if !verifier.verify(&signed.fact, &signed.signature) {
            return Err(Error::Invalid("the epoch's descriptor is not the hub's".into()));
        }
        let fact = EpochFact::decode(&signed.fact).map_err(Error::Invalid)?;
        if fact.scope != scope {
            return Err(Error::Invalid("the epoch is of another scope".into()));
        }
        let scope = scope.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 512 + signed.fact.len(), move |db| {
            let Some((scope_id, _, Some(_))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let stream = stream_of(db, scope_id, &fact.stream)?;
            let later: bool = db
                .prepare_cached("SELECT EXISTS (SELECT 1 FROM known_epochs WHERE stream = ?1 AND epoch > ?2)")?
                .query_row(params![stream, fact.epoch as i64], |row| row.get(0))?;
            // Opened, the epoch is written in from where this stream ends
            // here: one that begins further on waits until what comes before
            // it is here, and is not known here before, lest this device
            // order a stream it has not caught up with.
            let (next, head, current): (i64, Option<Vec<u8>>, i64) = db
                .prepare_cached("SELECT next, head, epoch FROM streams WHERE id = ?1")?
                .query_row([stream], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            if !later && (next as u64) < fact.start {
                return Ok(Taken::Behind);
            }
            // A successor's epoch opened here begins exactly where the stream
            // stands, chained to what stands: one this device wrote past, or
            // came to by another chain, is not opened (checked and opened in
            // this one transaction).
            if !later && (current.max(0) as u64) < fact.epoch && fact.predecessor.is_some() {
                if next as u64 != fact.start {
                    return Ok(Taken::Ahead);
                }
                if chain_value(head)? != fact.follows {
                    return Ok(Taken::Forked);
                }
            }
            // Written in here already under another descriptor of that epoch
            // — one this device opened as a refusal was settled, say — it is
            // another history: not taken.
            let written: Option<Vec<u8>> = db
                .prepare_cached("SELECT descriptor FROM epochs WHERE stream = ?1 AND epoch = ?2")?
                .query_row(params![stream, fact.epoch as i64], |row| row.get(0))
                .optional()?;
            if written.is_some_and(|written| EpochFact::decode(&written).ok().as_ref() != Some(&fact)) {
                return Ok(Taken::Forked);
            }
            match known_epoch(db, stream, fact.epoch)? {
                Some(kept) if kept != fact => return Err(invalid("another descriptor for that epoch is here")),
                Some(_) => {}
                None => {
                    db.prepare_cached("INSERT INTO known_epochs (stream, epoch, predecessor, fact, signature) VALUES (?1, ?2, ?3, ?4, ?5)")?
                        .execute(params![stream, fact.epoch as i64, fact.predecessor.map(|epoch| epoch as i64), signed.fact, signed.signature])?;
                }
            }
            if later {
                return Ok(Taken::History);
            }
            let opened = db
                .prepare_cached("UPDATE streams SET epoch = ?2 WHERE id = ?1 AND epoch < ?2")?
                .execute(params![stream, fact.epoch as i64])?;
            if opened == 1 {
                db.prepare_cached("INSERT INTO epochs (stream, epoch, descriptor) VALUES (?1, ?2, ?3)")?
                    .execute(params![stream, fact.epoch as i64, signed.fact])?;
            }
            Ok(Taken::Opened)
        })?)
    }

    /// Takes the hub's word, `signed`, on the epoch this device orders a
    /// stream of `scope` in, in one transaction: the epoch written in here,
    /// it is kept as the hub signed it — refused where another descriptor of
    /// it is kept or written in here; a later one is opened only where the
    /// stream stands exactly where it begins, chained to what it follows
    /// (but for a stream's first epoch, which begins at its start); an
    /// earlier one is refused.
    pub fn begin_epoch(&self, scope: &str, signed: Signed, verifier: &dyn HubVerifier) -> Result<Begun> {
        if !verifier.verify(&signed.fact, &signed.signature) {
            return Err(Error::Invalid("the epoch's descriptor is not the hub's".into()));
        }
        let fact = EpochFact::decode(&signed.fact).map_err(Error::Invalid)?;
        if fact.scope != scope {
            return Err(Error::Invalid("the epoch is of another scope".into()));
        }
        let scope = scope.to_string();
        Ok(self.database().write_now_at(Priority::Normal, 512 + signed.fact.len(), move |db| {
            let Some((scope_id, _, Some(_))) = held(db, &scope)? else { return Err(invalid("the scope is not held here")) };
            let stream = stream_of(db, scope_id, &fact.stream)?;
            let (next, head, current): (i64, Option<Vec<u8>>, i64) = db
                .prepare_cached("SELECT next, head, epoch FROM streams WHERE id = ?1")?
                .query_row([stream], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            let current = current.max(0) as u64;
            if let Some(kept) = known_epoch(db, stream, fact.epoch)?
                && kept != fact
            {
                return Ok(Begun::Refused(format!("another descriptor of epoch {} is kept here", fact.epoch)));
            }
            let written: Option<Vec<u8>> = db
                .prepare_cached("SELECT descriptor FROM epochs WHERE stream = ?1 AND epoch = ?2")?
                .query_row(params![stream, fact.epoch as i64], |row| row.get(0))
                .optional()?;
            if written.is_some_and(|written| EpochFact::decode(&written).ok().as_ref() != Some(&fact)) {
                return Ok(Begun::Refused(format!("epoch {} is written in here under another descriptor", fact.epoch)));
            }
            let opening = match fact.epoch.cmp(&current) {
                std::cmp::Ordering::Less => {
                    return Ok(Begun::Refused(format!("it is written in here in a later epoch, {current}")));
                }
                std::cmp::Ordering::Equal => false,
                std::cmp::Ordering::Greater => {
                    if fact.predecessor.is_some() {
                        if next as u64 != fact.start {
                            return Ok(Begun::Refused(format!("it stands at {next} here, not where the epoch begins")));
                        }
                        if chain_value(head)? != fact.follows {
                            return Ok(Begun::Refused("it reaches where the epoch begins by another chain here".into()));
                        }
                    }
                    true
                }
            };
            if known_epoch(db, stream, fact.epoch)?.is_none() {
                db.prepare_cached("INSERT INTO known_epochs (stream, epoch, predecessor, fact, signature) VALUES (?1, ?2, ?3, ?4, ?5)")?
                    .execute(params![stream, fact.epoch as i64, fact.predecessor.map(|epoch| epoch as i64), signed.fact, signed.signature])?;
            }
            if !opening {
                return Ok(Begun::InForce);
            }
            db.prepare_cached("UPDATE streams SET epoch = ?2 WHERE id = ?1")?.execute(params![stream, fact.epoch as i64])?;
            db.prepare_cached("INSERT OR REPLACE INTO epochs (stream, epoch, descriptor) VALUES (?1, ?2, ?3)")?
                .execute(params![stream, fact.epoch as i64, signed.fact])?;
            Ok(Begun::Opened)
        })?)
    }

    /// The epoch `stream` of `scope` is written in here, as the hub's signed
    /// word on it kept here says it — not merely some epoch of it, and the
    /// very descriptor it was written in under, where that is kept: none
    /// where there is no such word.
    pub fn epoch_in_force(&self, scope: &str, stream: &str) -> Result<Option<EpochFact>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, _)) = held(db, &scope)? else { return Ok(None) };
            let found: Option<(Vec<u8>, Option<Vec<u8>>)> = db
                .prepare_cached(
                    "SELECT k.fact, e.descriptor FROM streams s
                     JOIN known_epochs k ON k.stream = s.id AND k.epoch = s.epoch
                     LEFT JOIN epochs e ON e.stream = s.id AND e.epoch = s.epoch
                     WHERE s.scope = ?1 AND s.name = ?2",
                )?
                .query_row(params![scope_id, stream], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            let Some((known, written)) = found else { return Ok(None) };
            let Ok(known) = EpochFact::decode(&known) else { return Ok(None) };
            if written.is_some_and(|written| EpochFact::decode(&written).ok().as_ref() != Some(&known)) {
                return Ok(None);
            }
            Ok(Some(known))
        })?)
    }

    /// What this device, `device`, can release of the streams of `scope` it
    /// orders.
    pub fn releasable(&self, scope: &str, device: &[u8; 32]) -> Result<Vec<Releasable>> {
        let (scope, device) = (scope.to_string(), *device);
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, state, Some(holding))) = held(db, &scope)? else { return Ok(Vec::new()) };
            if state != "live" {
                return Ok(Vec::new());
            }
            let mut found = Vec::new();
            for (stream, name, next) in super::device::own(db, scope_id, holding, &device)? {
                let (epoch, head): (i64, Option<Vec<u8>>) = db
                    .prepare_cached("SELECT epoch, head FROM streams WHERE id = ?1")?
                    .query_row([stream], |row| Ok((row.get(0)?, row.get(1)?)))?;
                let uploaded: i64 = db
                    .prepare_cached("SELECT uploaded FROM outbox WHERE stream = ?1")?
                    .query_row([stream], |row| row.get(0))
                    .optional()?
                    .unwrap_or(0);
                let head: [u8; 32] = head.unwrap_or_else(|| vec![0; 32]).try_into().map_err(|_| invalid("a malformed chain value"))?;
                found.push(Releasable { stream: name, epoch: epoch as u64, next: next as u64, head, uploaded: uploaded as u64 });
            }
            Ok(found)
        })?)
    }

    /// The last epoch of `stream` of `scope` whose descriptor this device
    /// took; none before the first.
    pub fn known_last_epoch(&self, scope: &str, stream: &str) -> Result<Option<u64>> {
        let (scope, stream) = (scope.to_string(), stream.to_string());
        Ok(self.database().read_now(move |db| {
            let Some((scope_id, _, _)) = held(db, &scope)? else { return Ok(None) };
            let last: Option<i64> = db
                .prepare_cached(
                    "SELECT MAX(k.epoch) FROM known_epochs k JOIN streams s ON s.id = k.stream WHERE s.scope = ?1 AND s.name = ?2",
                )?
                .query_row(params![scope_id, stream], |row| row.get(0))?;
            Ok(last.map(|epoch| epoch as u64))
        })?)
    }
}
