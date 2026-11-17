//! Checking a stream against itself: its frames chain from the first to the
//! head the stream has, and applying them in order gives the index as it is.
//! The index is derived, so this is also what rebuilding it would give.

use crate::frame;
use crate::record::{self, State};
use crate::{Content, Result, StreamKey};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// What an audit went through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Audited {
    pub frames: u64,
    pub records: u64,
    pub items: u64,
}

/// An item as the audit holds it: where it stands, the record that set it,
/// and a digest in place of its state.
type Held = (Vec<u8>, u64, [u8; 32]);

impl Content {
    /// Reads the whole stream; for a patrol or a drill, on a thread that
    /// may block for as long as the stream is long.
    pub fn audit(&self, stream: &StreamKey) -> Result<Audited> {
        stream.check()?;
        Ok(self.0.database.read_background_now(|db| audit(db, stream))?)
    }
}

fn digest(body: &[u8], objects: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update((body.len() as u64).to_le_bytes());
    hasher.update(body);
    hasher.update(objects);
    hasher.finalize().into()
}

fn broken(what: impl Into<String>) -> fastctx_storage::Error {
    fastctx_storage::Error::new(what)
}

fn audit(db: &Connection, stream: &StreamKey) -> fastctx_storage::Result<Audited> {
    let snapshot = db.unchecked_transaction()?;
    let row: Option<(i64, i64, Option<Vec<u8>>)> = snapshot
        .prepare_cached(
            "SELECT streams.id, streams.next, streams.head FROM streams JOIN scopes ON scopes.id = streams.scope \
             WHERE scopes.key = ?1 AND streams.name = ?2",
        )?
        .query_row(params![stream.scope, stream.name], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    let Some((id, next, head)) = row else { return Ok(Audited { frames: 0, records: 0, items: 0 }) };

    let mut held: HashMap<Vec<u8>, Held> = HashMap::new();
    let (mut frames, mut sequence, mut chained) = (0u64, 0u64, None::<[u8; 32]>);
    {
        let mut stored =
            snapshot.prepare_cached("SELECT first, count, head, bytes FROM frames WHERE stream = ?1 ORDER BY first")?;
        let mut rows = stored.query([id])?;
        while let Some(row) = rows.next()? {
            let (first, count, said, bytes): (i64, u32, Vec<u8>, Vec<u8>) =
                (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
            if first as u64 != sequence {
                return Err(broken(format!("a frame starts at {first} where {sequence} follows")));
            }
            let (records, content) = frame::records(&bytes, count).map_err(broken)?;
            let head = frame::chain(chained.as_ref(), sequence, count, &content);
            if said != head {
                return Err(broken(format!("the frame at {first} does not chain to the one before it")));
            }
            chained = Some(head);
            for (offset, record) in records.into_iter().enumerate() {
                match record.state {
                    State::Is { body, objects } => {
                        let state = digest(&body, &record::pack(&objects));
                        held.insert(record.item, (record.order, sequence + offset as u64, state));
                    }
                    State::Gone => {
                        held.remove(&record.item);
                    }
                }
            }
            sequence += u64::from(count);
            frames += 1;
        }
    }
    if sequence != next as u64 || head.as_deref() != chained.as_ref().map(|head| &head[..]) {
        return Err(broken("the stream's head is not its last frame's"));
    }

    let mut items = 0u64;
    let mut indexed =
        snapshot.prepare_cached("SELECT place, item, sequence, body, objects FROM items WHERE stream = ?1")?;
    let mut rows = indexed.query([id])?;
    while let Some(row) = rows.next()? {
        let (place, item, set, body, objects): (Vec<u8>, Vec<u8>, i64, Vec<u8>, Vec<u8>) =
            (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?);
        if held.get(&item) != Some(&(place, set as u64, digest(&body, &objects))) {
            return Err(broken("the index holds an item its frames do not give"));
        }
        items += 1;
    }
    if items != held.len() as u64 {
        return Err(broken(format!("the frames give {} items and the index holds {items}", held.len())));
    }
    Ok(Audited { frames, records: sequence, items })
}
