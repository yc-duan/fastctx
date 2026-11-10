//! Appending a batch: its frames, the index and the object counts change in
//! one transaction, or none of them does.

use crate::error::invalid;
use crate::frame::{self, Frame};
use crate::record::{self, ObjectRef, Record, State};
use crate::{Content, Error, NAME_BYTES, Result, StreamKey};
use fastctx_storage::Priority;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

/// The most bytes one batch brings; its producer splits what is larger.
pub const BATCH_BYTES: usize = 8 * 1024 * 1024;
/// The largest state a producer keeps with its records.
pub const CHECKPOINT_BYTES: usize = 64 * 1024;

/// A producer's own position in what it reads, kept with the records it
/// made from it or not at all.
pub struct Checkpoint {
    pub name: String,
    pub state: Vec<u8>,
}

pub struct Batch {
    pub stream: StreamKey,
    /// The generation its producer sequences the stream under. A batch of a
    /// generation the stream has left is refused.
    pub generation: u64,
    pub records: Vec<Record>,
    pub checkpoint: Option<Checkpoint>,
    /// Whether the batch reaches the disk before it is answered. What its
    /// producer can make again from its source need not.
    pub durable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appended {
    /// The stream sequence of the batch's first record.
    pub first: u64,
    pub count: u32,
    /// The stream's revision after the batch.
    pub revision: u64,
}

impl Content {
    /// Appends a batch to its stream and applies it to the index. It blocks
    /// until the batch is committed: call on a thread that may block, or a
    /// task thread, which waits in place.
    pub fn append(&self, batch: Batch) -> Result<Appended> {
        batch.stream.check()?;
        for record in &batch.records {
            record.check().map_err(Error::Invalid)?;
        }
        if let Some(checkpoint) = &batch.checkpoint
            && (checkpoint.name.is_empty()
                || checkpoint.name.len() > NAME_BYTES
                || checkpoint.state.len() > CHECKPOINT_BYTES)
        {
            return Err(Error::Invalid(format!(
                "a checkpoint is named in 1 to {NAME_BYTES} bytes and holds at most {CHECKPOINT_BYTES}"
            )));
        }
        let frames = frame::frames(&batch.records).map_err(Error::Invalid)?;
        let bytes = batch.records.iter().map(Record::encoded_len).sum::<usize>()
            + frames.iter().map(|frame| frame.bytes.len()).sum::<usize>()
            + batch.checkpoint.as_ref().map_or(0, |checkpoint| checkpoint.state.len());
        if bytes > BATCH_BYTES {
            return Err(Error::Invalid(format!("a batch brings at most {BATCH_BYTES} bytes; this one brings {bytes}")));
        }
        let Batch { stream, generation, records, checkpoint, durable } = batch;
        let key = stream.clone();
        let write = move |db: &Connection| apply(db, &stream, generation, &frames, &records, checkpoint.as_ref());
        let appended = if durable {
            self.0.database.write_now_at(Priority::Normal, bytes, write)?
        } else {
            self.0.database.write_relaxed_now_at(Priority::Normal, bytes, write)?
        };
        if appended.count > 0 {
            self.0.wakes.moved(&key, appended.revision);
        }
        Ok(appended)
    }

    /// Makes every batch appended so far as durable as any other, before
    /// something is done that only they would undo, as sending them on.
    pub fn settle(&self) -> Result<()> {
        Ok(self.0.database.settle_now(Priority::Normal)?)
    }
}

struct Stream {
    id: i64,
    generation: u64,
    next: u64,
    head: Option<[u8; 32]>,
    revision: u64,
}

fn apply(
    db: &Connection,
    stream: &StreamKey,
    generation: u64,
    frames: &[Frame],
    records: &[Record],
    checkpoint: Option<&Checkpoint>,
) -> fastctx_storage::Result<Appended> {
    let scope = scope(db, &stream.scope)?;
    let known = stream_row(db, scope, &stream.name)?;
    if generation < known.generation {
        return Err(invalid(format!(
            "the stream is sequenced under generation {}; a batch of generation {generation} is not taken",
            known.generation
        )));
    }
    let first = known.next;
    let mut next = first;
    let mut head = known.head;
    if !frames.is_empty() {
        let mut insert = db.prepare_cached(
            "INSERT INTO frames (stream, first, count, generation, head, bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for frame in frames {
            let chained = frame::chain(head.as_ref(), next, frame.count, &frame.digest);
            insert.execute(params![
                known.id,
                signed(next)?,
                frame.count,
                signed(generation)?,
                &chained[..],
                frame.bytes
            ])?;
            head = Some(chained);
            next += u64::from(frame.count);
        }
    }
    let counts = index(db, known.id, first, records)?;
    count(db, scope, counts)?;
    let revision = if records.is_empty() { known.revision } else { known.revision + 1 };
    db.prepare_cached("UPDATE streams SET generation = ?2, next = ?3, head = ?4, revision = ?5 WHERE id = ?1")?
        .execute(params![
            known.id,
            signed(generation)?,
            signed(next)?,
            head.as_ref().map(|head| &head[..]),
            signed(revision)?
        ])?;
    if let Some(checkpoint) = checkpoint {
        db.prepare_cached(
            "INSERT INTO checkpoints (name, state) VALUES (?1, ?2) \
             ON CONFLICT (name) DO UPDATE SET state = excluded.state",
        )?
        .execute(params![checkpoint.name, checkpoint.state])?;
    }
    Ok(Appended { first, count: records.len() as u32, revision })
}

fn scope(db: &Connection, key: &str) -> fastctx_storage::Result<i64> {
    let known: Option<i64> =
        db.prepare_cached("SELECT id FROM scopes WHERE key = ?1")?.query_row([key], |row| row.get(0)).optional()?;
    if let Some(id) = known {
        return Ok(id);
    }
    db.prepare_cached("INSERT INTO scopes (key) VALUES (?1)")?.execute([key])?;
    Ok(db.last_insert_rowid())
}

fn stream_row(db: &Connection, scope: i64, name: &str) -> fastctx_storage::Result<Stream> {
    let known = db
        .prepare_cached("SELECT id, generation, next, head, revision FROM streams WHERE scope = ?1 AND name = ?2")?
        .query_row(params![scope, name], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .optional()?;
    let Some((id, generation, next, head, revision)) = known else {
        db.prepare_cached("INSERT INTO streams (scope, name) VALUES (?1, ?2)")?.execute(params![scope, name])?;
        return Ok(Stream { id: db.last_insert_rowid(), generation: 0, next: 0, head: None, revision: 0 });
    };
    let head = match head {
        Some(head) => Some(
            <[u8; 32]>::try_from(head.as_slice())
                .map_err(|_| fastctx_storage::Error::new("a stream's head is not a SHA-256"))?,
        ),
        None => None,
    };
    Ok(Stream { id, generation: generation as u64, next: next as u64, head, revision: revision as u64 })
}

/// Applies records to the index, and answers how the count of records that
/// refer to each object changed.
fn index(
    db: &Connection,
    stream: i64,
    first: u64,
    records: &[Record],
) -> fastctx_storage::Result<HashMap<ObjectRef, i64>> {
    let mut counts = HashMap::new();
    if records.is_empty() {
        return Ok(counts);
    }
    let mut find = db.prepare_cached("SELECT place, objects FROM items WHERE stream = ?1 AND item = ?2")?;
    let mut remove = db.prepare_cached("DELETE FROM items WHERE stream = ?1 AND place = ?2 AND item = ?3")?;
    let mut put = db.prepare_cached(
        "INSERT INTO items (stream, place, item, sequence, body, objects) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (stream, place, item) DO UPDATE SET \
         sequence = excluded.sequence, body = excluded.body, objects = excluded.objects",
    )?;
    for (offset, record) in records.iter().enumerate() {
        let was: Option<(Vec<u8>, Vec<u8>)> =
            find.query_row(params![stream, record.item], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
        if let Some((place, objects)) = &was {
            for object in record::unpack(objects).map_err(fastctx_storage::Error::new)? {
                *counts.entry(object).or_insert(0) -= 1;
            }
            let stays = matches!(&record.state, State::Is { .. }) && *place == record.order;
            if !stays {
                remove.execute(params![stream, place, record.item])?;
            }
        }
        if let State::Is { body, objects } = &record.state {
            for object in objects {
                *counts.entry(*object).or_insert(0) += 1;
            }
            put.execute(params![
                stream,
                record.order,
                record.item,
                signed(first + offset as u64)?,
                body,
                record::pack(objects)
            ])?;
        }
    }
    Ok(counts)
}

fn count(db: &Connection, scope: i64, counts: HashMap<ObjectRef, i64>) -> fastctx_storage::Result<()> {
    if counts.values().all(|change| *change == 0) {
        return Ok(());
    }
    let mut add = db.prepare_cached(
        "INSERT INTO object_refs (scope, hash, size, count) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (scope, hash) DO UPDATE SET count = count + excluded.count",
    )?;
    let mut clear = db.prepare_cached("DELETE FROM object_refs WHERE scope = ?1 AND hash = ?2 AND count <= 0")?;
    for (object, change) in counts {
        if change == 0 {
            continue;
        }
        add.execute(params![scope, &object.hash[..], signed(object.size)?, change])?;
        if change < 0 {
            clear.execute(params![scope, &object.hash[..]])?;
        }
    }
    Ok(())
}

fn signed(value: u64) -> fastctx_storage::Result<i64> {
    i64::try_from(value).map_err(|_| invalid("a sequence is beyond what the database counts"))
}
