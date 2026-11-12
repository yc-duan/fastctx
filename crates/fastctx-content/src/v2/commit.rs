//! A producer's commit (§4.2): one transaction checks the scope is held and
//! the holding period current, every object referred to present at its
//! length (I3), packs the records into signed frames, writes them with
//! their object references, applies them through the one index entry
//! (§2.4), advances the stream and lets go of the leases they used.
//! A true source's commit waits for the disk; a projection's does not (§3).

use super::Library;
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
fn stream_of(db: &Connection, scope: i64, name: &str) -> fastctx_storage::Result<(i64, u64, u64, [u8; 32])> {
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

impl Library {
    /// Commits `commit` as the stream's ordering device, signing each frame
    /// with `signer`.
    pub fn commit(&self, commit: Commit, signer: Arc<dyn Signer>) -> Result<Committed> {
        for record in &commit.records {
            record.record.check().map_err(Error::Invalid)?;
        }
        if commit.records.is_empty() {
            return Err(Error::Invalid("a commit holds at least one record".into()));
        }
        let sealed = seal(&commit.records).map_err(Error::Invalid)?;
        let bytes = sealed.iter().map(|frame| frame.body.len() + 256).sum::<usize>() + commit.records.len() * 64;
        let durable = commit.durable;
        let write = move |db: &Connection| -> fastctx_storage::Result<Committed> {
            let Some((scope, state, Some(holding))) = held(db, &commit.scope)? else {
                return Err(invalid("the scope is not held here"));
            };
            if state != "live" && state != "provisional" {
                return Err(invalid("the scope is not held here"));
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
            let (stream, epoch, next, mut head) = stream_of(db, scope, &commit.stream)?;
            if let Some((op, _)) = &commit.op
                && super::ops::seen(db, op)?
            {
                return Ok(Committed { first: next, count: 0, frames: 0, head, duplicate: true });
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
            if let Some((op, words)) = &commit.op {
                let mut record = commit.stream.as_bytes().to_vec();
                record.push(0);
                record.extend_from_slice(&commit.records[0].record.item);
                super::ops::landed(db, op, &record, *words)?;
            }
            Ok(Committed { first: next, count: first - next, frames: sealed.len(), head, duplicate: false })
        };
        Ok(if durable {
            self.database().write_now_at(Priority::Normal, bytes, write)?
        } else {
            self.database().write_relaxed_now_at(Priority::Normal, bytes, write)?
        })
    }
}
