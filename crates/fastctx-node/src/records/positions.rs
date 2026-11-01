//! Published cursors belong to a conversation and never reuse a deleted row's
//! number. Moving a conversation preserves both its rows and its high watermarks.
use fastctx_storage::{
    Error, Result,
    rusqlite::{Connection, OptionalExtension},
};

pub(crate) fn next(db: &Connection, conversation: &str, stream: &str) -> Result<i64> {
    db.prepare_cached(
        "INSERT INTO conversation_record_positions(conversation,stream,position) VALUES(?1,?2,1)
        ON CONFLICT(conversation,stream) DO UPDATE SET position=position+1 WHERE position<9223372036854775807
        RETURNING position",
    )?
    .query_row((conversation, stream), |row| row.get(0))
    .optional()?
    .ok_or_else(|| Error::new("conversation record position exhausted"))
}
