//! A producer's commit (§4.2): one transaction checks the scope is held and
//! the holding period current, every object referred to present at its
//! length (I3), packs the records into signed frames, writes them with
//! their object references, applies them through the one index entry
//! (§2.4), advances the stream and lets go of the leases they used.
//! A true source's commit waits for the disk; a projection's does not (§3).

use super::{Entry, Library};
use super::frame::{Header, KindRecord, Signer, seal};
use super::items::{Applying, apply, floors_passed};
use super::objects::{Lease, present, release_in};
use super::scopes::held;
use crate::error::invalid;
use crate::record::State;
use crate::{Error, Result};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;
use std::sync::Arc;

/// What a producer commits to one stream.
pub struct Commit {
    pub scope: String,
    pub stream: String,
    pub records: Vec<KindRecord>,
    /// The leases on the objects the records refer to, let go with the
    /// commit.
    pub leases: Vec<Lease>,
    /// A true source's write waits for the disk; a projection's does not.
    pub durable: bool,
    /// The operation a true source's write carries (§3), and whether its
    /// records are words for a host.
    pub op: Option<(super::ops::Op, bool)>,
    /// The holding period the records were made in (`Library::holding`): a
    /// commit whose period ended meanwhile writes nothing (§4.5).
    pub holding: i64,
}

/// Where a commit left its stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committed {
    pub first: u64,
    pub count: u64,
    pub frames: usize,
    pub head: [u8; 32],
    /// The operation was committed before: nothing was written again.
    pub duplicate: bool,
}

/// The stream's id, epoch, next sequence and chain value, made at need.
pub(super) fn stream_of(db: &Connection, scope: i64, name: &str) -> fastctx_storage::Result<(i64, u64, u64, [u8; 32])> {
    let found: Option<(i64, i64, i64, Option<Vec<u8>>)> = db
        .prepare_cached("SELECT id, epoch, next, head FROM streams WHERE scope = ?1 AND name = ?2")?
        .query_row(params![scope, name], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .optional()?;
    if let Some((id, epoch, next, head)) = found {
        let head = head.map_or(Ok([0; 32]), |head| head.try_into()).map_err(|_| invalid("a malformed chain value"))?;
        return Ok((id, epoch as u64, next as u64, head));
    }
    db.prepare_cached("INSERT INTO streams (scope, name, epoch, next, head, complete_through) VALUES (?1, ?2, 0, 0, NULL, NULL)")?
        .execute(params![scope, name])?;
    Ok((db.last_insert_rowid(), 0, 0, [0; 32]))
}

/// A kind's rule, judging each record against its item as it stands in the
/// transaction that commits it (`Writer`).
pub(super) type Judge = Box<dyn Fn(Option<&Entry>, &KindRecord) -> std::result::Result<(), String> + Send>;

impl Library {
    /// Commits `commit` unjudged, for the drills alone: everything else
    /// commits through a `Writer`, which judges each record, or is the
    /// library's own recommit.
    #[cfg(any(test, feature = "drill"))]
    #[doc(hidden)]
    pub fn raw_commit(&self, commit: Commit, signer: Arc<dyn Signer>) -> Result<Committed> {
        self.commit_then(commit, signer, None, |_, _| Ok(()))
    }

    /// Commits as `commit` does, each record judged by `judge` against its
    /// item as it stands in the same transaction, the records before it in
    /// the commit applied (§11).
    pub(super) fn commit_judged(&self, commit: Commit, signer: Arc<dyn Signer>, judge: Judge) -> Result<Committed> {
        self.commit_then(commit, signer, Some(judge), |_, _| Ok(()))
    }

    /// Commits as `commit` does, judged by `judge` if given, with `then` done
    /// in the same transaction once the records are in.
    pub(super) fn commit_then(
        &self,
        commit: Commit,
        signer: Arc<dyn Signer>,
        judge: Option<Judge>,
        then: impl FnOnce(&Connection, u64) -> fastctx_storage::Result<()> + Send + 'static,
    ) -> Result<Committed> {
        for record in &commit.records {
            record.record.check().map_err(Error::Invalid)?;
        }
        if commit.records.is_empty() {
            return Err(Error::Invalid("a commit holds at least one record".into()));
        }
        let sealed = seal(&commit.records).map_err(Error::Invalid)?;
        let bytes = sealed.iter().map(|frame| frame.body.len() + 256).sum::<usize>() + commit.records.len() * 64;
        let durable = commit.durable;
        let scope_key = commit.scope.clone();
        let write = move |db: &Connection| -> fastctx_storage::Result<Committed> {
            let Some((scope, state, Some(holding))) = held(db, &commit.scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" && state != "provisional" {
                return Err(invalid("the scope is not held here"));
            }
            if holding != commit.holding {
                return Err(invalid("the holding period the records were made in has ended"));
            }
            let (stream, epoch, next, mut head) = stream_of(db, scope, &commit.stream)?;
            // An operation done before is answered as it was, before anything
            // is judged again: the item may have moved on since (§3).
            if let Some((op, _)) = &commit.op
                && super::ops::seen(db, op)?
            {
                return Ok(Committed { first: next, count: 0, frames: 0, head, duplicate: true });
            }
            if super::letgo::withheld(db, scope, &commit.stream, holding)? {
                return Err(invalid("this device may hold the stream no more"));
            }
            for record in &commit.records {
                if let State::Is { objects, .. } = &record.record.state {
                    for object in objects {
                        match present(db, &object.hash)? {
                            Some((_, size)) if size == object.size => {}
                            _ => return Err(invalid("a record refers to an object that is not here at its length")),
                        }
                    }
                }
            }
            if let Some(judge) = &judge {
                let mut earlier: std::collections::BTreeMap<Vec<u8>, Option<Entry>> = std::collections::BTreeMap::new();
                for (offset, record) in commit.records.iter().enumerate() {
                    let item = &record.record.item;
                    let prior = match earlier.remove(item) {
                        Some(prior) => prior,
                        None => super::hub::before(db, stream, holding, item)?,
                    };
                    judge(prior.as_ref(), record).map_err(invalid)?;
                    earlier.insert(item.clone(), super::hub::after(record, next + offset as u64));
                }
            }
            let at = Applying { scope, stream, holding };
            let mut first = next;
            for frame in &sealed {
                let header = Header {
                    scope: commit.scope.clone(),
                    stream: commit.stream.clone(),
                    epoch,
                    first,
                    count: frame.count,
                    previous: head,
                    digest: frame.digest,
                    device: signer.device(),
                    key: signer.key(),
                };
                let encoded = header.encode();
                let signature = signer.sign(&encoded);
                head = header.chain();
                db.prepare_cached(
                    "INSERT INTO frames (stream, epoch, first, holding, count, head, header, signature, body, change)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
                )?
                .execute(params![
                    stream,
                    epoch as i64,
                    first as i64,
                    holding,
                    frame.count,
                    &head[..],
                    encoded,
                    signature,
                    frame.body
                ])?;
                let mut hashes = BTreeSet::new();
                for record in &commit.records[frame.records.clone()] {
                    if let State::Is { objects, .. } = &record.record.state {
                        hashes.extend(objects.iter().map(|object| object.hash));
                    }
                }
                let mut referring = db.prepare_cached(
                    "INSERT OR IGNORE INTO frame_objects (stream, epoch, first, holding, hash) VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                for hash in &hashes {
                    referring.execute(params![stream, epoch as i64, first as i64, holding, &hash[..]])?;
                }
                for (offset, record) in commit.records[frame.records.clone()].iter().enumerate() {
                    apply(db, at, first + offset as u64, record)?;
                }
                first += u64::from(frame.count);
            }
            db.prepare_cached("UPDATE streams SET next = ?2, head = ?3 WHERE id = ?1")?
                .execute(params![stream, first as i64, &head[..]])?;
            db.prepare_cached(
                "INSERT INTO cursors (stream, holding, sequence, head) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (stream, holding) DO UPDATE SET sequence = excluded.sequence, head = excluded.head",
            )?
            .execute(params![stream, holding, first as i64 - 1, &head[..]])?;
            floors_passed(db, stream, first - 1)?;
            release_in(db, &commit.leases)?;
            // An operation that stood at these places before a truncation
            // stands there no more: only this commit's own land here.
            db.prepare_cached("DELETE FROM op_records WHERE scope = ?1 AND stream = ?2 AND revision >= ?3 AND revision < ?4")?
                .execute(params![scope, commit.stream, next as i64, first as i64])?;
            db.prepare_cached("DELETE FROM resubmit_order WHERE stream = ?1 AND holding = ?2 AND revision >= ?3 AND revision < ?4")?
                .execute(params![stream, commit.holding, next as i64, first as i64])?;
            then(db, next)?;
            if let Some((op, words)) = &commit.op {
                super::ops::landed(db, op, scope, &commit.stream, next, first - next, *words)?;
            }
            Ok(Committed { first: next, count: first - next, frames: sealed.len(), head, duplicate: false })
        };
        let committed = if durable {
            self.database().write_now_at(Priority::Normal, bytes, write)?
        } else {
            self.database().write_relaxed_now_at(Priority::Normal, bytes, write)?
        };
        if !committed.duplicate {
            self.0.committed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(scope_key);
        }
        self.wrote();
        Ok(committed)
    }
}
