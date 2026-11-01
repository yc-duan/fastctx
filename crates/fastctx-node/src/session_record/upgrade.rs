//! Retire the development staging table only after its durable events are
//! present in the canonical table. Original legacy files remain for verification.
use super::{search, sql};
use fastctx_storage::{
    Error, ErrorKind, Result,
    rusqlite::{Connection, OptionalExtension, params},
    versions::{self, Migration},
};
use fastctx_types::session::SessionEvent;

pub(crate) fn apply(db: &Connection) -> Result<()> {
    versions::apply(
        db,
        "node.session-events",
        &[
            Migration { version: 1, sql: sql::SCHEMA },
            Migration { version: 2, sql: sql::SCHEMA },
            Migration { version: 3, sql: sql::SCHEMA },
        ],
    )?;
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='session_legacy')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    let mut query=db.prepare("SELECT conversation,identity,sequenced,sequence,body FROM session_legacy ORDER BY conversation,sequenced,sequence,seconds,nanos,identity")?;
    let mut rows = query.query([])?;
    let mut scope = String::new();
    let mut reserved = 0;
    while let Some(row) = rows.next()? {
        let conversation: String = row.get(0)?;
        let identity: String = row.get(1)?;
        let sequenced: bool = row.get(2)?;
        let original: Vec<u8> = row.get(3)?;
        let body: Vec<u8> = row.get(4)?;
        if scope != conversation {
            scope = conversation.clone();
            let source: Option<Vec<u8>> = db.query_row(
                "SELECT max(sequence) FROM session_legacy WHERE conversation=?1 AND sequenced=1",
                [&scope],
                |row| row.get(0),
            )?;
            reserved = sql::head(db, &scope)?.0.max(source.map(sql::number).transpose()?.unwrap_or(0));
        }
        let previous: Option<Vec<u8>> = db
            .query_row(
                "SELECT body FROM session_events WHERE conversation=?1 AND identity=?2",
                (&conversation, &identity),
                |row| row.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            if previous != body {
                return Err(Error::with_kind(
                    ErrorKind::Corrupt,
                    "legacy SQL event conflicts with its canonical identity",
                ));
            }
            continue;
        }
        let event: SessionEvent =
            serde_json::from_slice(&body).map_err(|error| Error::with_kind(ErrorKind::Corrupt, error.to_string()))?;
        let wanted = if sequenced { sql::number(original)? } else { 0 };
        let used: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_events WHERE conversation=?1 AND sequence=?2)",
            (&conversation, wanted.to_be_bytes().as_slice()),
            |row| row.get(0),
        )?;
        let sequence = if wanted > 0 && !used {
            wanted
        } else {
            reserved.checked_add(1).ok_or_else(|| Error::new("session event sequence exhausted"))?
        };
        reserved = reserved.max(sequence);
        db.execute(
            "INSERT INTO session_events(conversation,sequence,identity,body) VALUES(?1,?2,?3,?4)",
            params![conversation, sequence.to_be_bytes().as_slice(), identity, body],
        )?;
        let (next, latest, _) = sql::head(db, &conversation)?;
        search::appended(db, &conversation, latest, sequence, &search::Tags::of(&event))?;
        sql::set_head(db, &conversation, next.max(reserved), latest.max(sequence), false)?;
    }
    drop(rows);
    drop(query);
    // Previously imported families must recheck and archive the original files.
    db.execute(
        "UPDATE session_heads SET imported=0 WHERE conversation IN (SELECT conversation FROM session_legacy)",
        [],
    )?;
    db.execute_batch("DROP TABLE session_legacy")?;
    Ok(())
}
