//! Reading what was appended: a stream's records from a place in its
//! sequence on, in the order they were appended. Where the index says how
//! each item stands now, this says what came since a reader last read, so a
//! reader that holds a part of a stream brings that part up to date without
//! reading it again. It reads the frames, never the index, and costs what
//! came since, not what the stream holds.

use crate::frame;
use crate::record::{Record, State};
use crate::{Content, Error, Result, StreamKey};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

/// A record as it was appended, with its number in the stream's sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub sequence: u64,
    pub record: Record,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// The stream's revision as its records were read; none when the stream
    /// is not here.
    pub revision: Option<u64>,
    /// In the order they were appended. An item may be among them more than
    /// once: its last record is how it stands.
    pub changes: Vec<Change>,
    /// Where to read on from: the sequence after the last of these, or the
    /// one asked from when there are none.
    pub through: u64,
    /// Whether the stream holds records after these.
    pub more: bool,
}

/// What a record is reckoned to take in an answer besides its body.
const RECORD: usize = 128;

impl Content {
    /// The records appended to a stream from sequence `from` on: at most
    /// `max_records` of them and `max_bytes` together, one at least. None
    /// when the stream does not hold its records from there on, as one that
    /// was forgotten and made again, or a place it never reached: the reader
    /// reads the stream's items anew. It blocks for the read: call on a
    /// thread that may block, or a task thread, which waits in place.
    pub fn changes(&self, stream: &StreamKey, from: u64, max_records: u32, max_bytes: u32) -> Result<Option<Changes>> {
        stream.check()?;
        if max_records == 0 {
            return Err(Error::Invalid("changes are read one record at least".into()));
        }
        Ok(self.0.database.read_now(|db| changes(db, stream, from, max_records as usize, max_bytes as usize))?)
    }
}

fn changes(
    db: &Connection,
    stream: &StreamKey,
    from: u64,
    max_records: usize,
    max_bytes: usize,
) -> fastctx_storage::Result<Option<Changes>> {
    // The stream's revision and its records are read as of one moment.
    let snapshot = db.unchecked_transaction()?;
    let known: Option<(i64, i64, i64)> = snapshot
        .prepare_cached(
            "SELECT streams.id, streams.revision, streams.next FROM streams JOIN scopes ON scopes.id = streams.scope \
             WHERE scopes.key = ?1 AND streams.name = ?2",
        )?
        .query_row(params![stream.scope, stream.name], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    let Some((id, revision, next)) = known else {
        // A stream that is not here has no records: none came, to a reader
        // that held none of it.
        return Ok((from == 0).then(Changes::default));
    };
    let next = next as u64;
    if from > next {
        return Ok(None);
    }
    let mut read = Changes { revision: Some(revision as u64), changes: Vec::new(), through: from, more: false };
    if from == next {
        return Ok(Some(read));
    }
    // The frame that holds the sequence asked from, and those after it.
    let first: Option<i64> = snapshot
        .prepare_cached("SELECT MAX(first) FROM frames WHERE stream = ?1 AND first <= ?2")?
        .query_row(params![id, from as i64], |row| row.get(0))?;
    let Some(first) = first else { return Ok(None) };
    let mut frames = snapshot
        .prepare_cached("SELECT first, count, bytes FROM frames WHERE stream = ?1 AND first >= ?2 ORDER BY first")?;
    let mut rows = frames.query(params![id, first])?;
    let mut expected = first as u64;
    let mut bytes = 0usize;
    'frames: while let Some(row) = rows.next()? {
        let (at, count, packed): (i64, u32, Vec<u8>) = (row.get(0)?, row.get(1)?, row.get(2)?);
        if at as u64 != expected {
            // Frames between are gone: the records from there are not all here.
            return Ok(None);
        }
        let (records, _) = frame::records(&packed, count).map_err(fastctx_storage::Error::new)?;
        for (offset, record) in records.into_iter().enumerate() {
            let sequence = expected + offset as u64;
            if sequence < from {
                continue;
            }
            let size = RECORD
                + match &record.state {
                    State::Is { body, .. } => body.len(),
                    State::Gone => 0,
                };
            if read.changes.len() == max_records || (!read.changes.is_empty() && bytes + size > max_bytes) {
                read.more = true;
                break 'frames;
            }
            bytes += size;
            read.through = sequence + 1;
            read.changes.push(Change { sequence, record });
        }
        expected += u64::from(count);
    }
    // The stream counts records its frames no longer hold.
    if !read.more && read.through != next {
        return Ok(None);
    }
    Ok(Some(read))
}
