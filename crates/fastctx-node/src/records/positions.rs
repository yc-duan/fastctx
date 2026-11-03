//! Published cursors belong to a conversation and never reuse a deleted row's
//! number. Moving a conversation preserves both its rows and its high watermarks.
use fastctx_storage::{
    Error, Result,
    rusqlite::{Connection, OptionalExtension},
};

pub(crate) fn next(db: &Connection, conversation: &str, stream: &str) -> Result<i64> {
    reserve(db, conversation, stream, 1)
}

/// Takes `count` consecutive positions of `stream` at once, for rows written
/// together, and gives the first; those left unused are never given again.
pub(crate) fn reserve(db: &Connection, conversation: &str, stream: &str, count: i64) -> Result<i64> {
    db.prepare_cached(
        "INSERT INTO conversation_record_positions(conversation,stream,position) VALUES(?1,?2,?3)
        ON CONFLICT(conversation,stream) DO UPDATE SET position=position+?3 WHERE position<=9223372036854775807-?3
        RETURNING position",
    )?
    .query_row((conversation, stream, count), |row| row.get::<_, i64>(0))
    .optional()?
    .map(|last| last - count + 1)
    .ok_or_else(|| Error::new("conversation record position exhausted"))
}
