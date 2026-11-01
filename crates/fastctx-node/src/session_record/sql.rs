//! A conversation's event order and idempotency keys commit together.

use fastctx_storage::{
    Error, Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_search_heads (conversation TEXT PRIMARY KEY, sequence BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS session_event_tags (
    conversation TEXT NOT NULL,tag TEXT NOT NULL,value TEXT NOT NULL,sequence BLOB NOT NULL,arrival BLOB NOT NULL,
    PRIMARY KEY(conversation,tag,value,sequence)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_event_tag_arrival ON session_event_tags(conversation,tag,value,arrival,sequence);
CREATE TABLE IF NOT EXISTS session_heads (
    conversation TEXT PRIMARY KEY, allocated BLOB NOT NULL, latest BLOB NOT NULL,
    imported INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS session_events (
    conversation TEXT NOT NULL, sequence BLOB NOT NULL, identity TEXT NOT NULL, body BLOB NOT NULL,
    PRIMARY KEY(conversation,sequence), UNIQUE(conversation,identity)
) WITHOUT ROWID;

";

pub(super) fn number(bytes: Vec<u8>) -> Result<u64> {
    Ok(u64::from_be_bytes(bytes.try_into().map_err(|_| Error::new("invalid event sequence"))?))
}

pub(super) fn head(db: &Connection, conversation: &str) -> Result<(u64, u64, bool)> {
    let row: Option<(Vec<u8>, Vec<u8>, bool)> = db
        .query_row("SELECT allocated,latest,imported FROM session_heads WHERE conversation=?1", [conversation], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    row.map(|(allocated, latest, imported)| Ok((number(allocated)?, number(latest)?, imported)))
        .unwrap_or(Ok((0, 0, false)))
}

pub(super) fn set_head(db: &Connection, conversation: &str, allocated: u64, latest: u64, imported: bool) -> Result<()> {
    db.execute("INSERT INTO session_heads(conversation,allocated,latest,imported) VALUES(?1,?2,?3,?4)
        ON CONFLICT(conversation) DO UPDATE SET allocated=excluded.allocated,latest=excluded.latest,imported=excluded.imported",
        params![conversation,allocated.to_be_bytes().as_slice(),latest.to_be_bytes().as_slice(),imported])?;
    Ok(())
}

pub(super) fn append(
    db: &Connection,
    conversation: &str,
    key: &str,
    body: &[u8],
    tags: &super::search::Tags,
) -> Result<bool> {
    let exists = db
        .query_row("SELECT 1 FROM session_events WHERE conversation=?1 AND identity=?2", (conversation, key), |row| {
            row.get::<_, i32>(0)
        })
        .optional()?
        .is_some();
    if exists {
        return Ok(false);
    }
    let (previous, latest, imported) = head(db, conversation)?;
    let sequence = previous.checked_add(1).ok_or_else(|| Error::new("session event sequence exhausted"))?;
    db.execute(
        "INSERT INTO session_events(conversation,sequence,identity,body) VALUES(?1,?2,?3,?4)",
        params![conversation, sequence.to_be_bytes().as_slice(), key, body],
    )?;
    super::search::appended(db, conversation, latest, sequence, tags)?;
    set_head(db, conversation, sequence, sequence, imported)?;
    Ok(true)
}
