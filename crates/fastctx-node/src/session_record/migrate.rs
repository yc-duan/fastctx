//! Legacy events are staged and sorted in SQLite, never materialized as one
//! conversation-sized vector. A restart safely repeats the uncommitted phase.

use super::{Recorded, sql};
use fastctx_storage::{
    Database, Priority,
    rusqlite::{OptionalExtension, params},
};
use fastctx_types::layout::ConversationDir;
use std::{io, sync::LazyLock};

static GATES: LazyLock<crate::record_gates::Gates> = LazyLock::new(|| {
    crate::record_gates::Gates::new(fastctx_platform::doing::Rank::RECORDS, "migrating session events")
});
const BATCH: usize = 16;

type Legacy = (String, bool, Vec<u8>, i64, i32, Vec<u8>);

pub(super) fn ensure(directory: &ConversationDir, database: &Database, conversation: &str) -> io::Result<()> {
    if database
        .read_now(|db| Ok(crate::conversation_store::deleted(db, conversation)? || sql::head(db, conversation)?.2))
        .map_err(io::Error::other)?
    {
        return Ok(());
    }
    let gate = GATES.get(directory.path());
    let _gate = gate.lock();
    if database
        .read_now(|db| Ok(crate::conversation_store::deleted(db, conversation)? || sql::head(db, conversation)?.2))
        .map_err(io::Error::other)?
    {
        return Ok(());
    }
    let mut batch = Vec::new();
    let entries = match std::fs::read_dir(directory.session_events()) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(entries) = entries {
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json")
                || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            let record: Recorded = serde_json::from_slice(&std::fs::read(&path)?)?;
            let key = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or_else(|| io::Error::other("invalid legacy event identity"))?
                .to_string();
            batch.push((
                key,
                record.sequence.is_some(),
                record.sequence.unwrap_or(0).to_be_bytes().to_vec(),
                record.event.at.as_second(),
                record.event.at.subsec_nanosecond(),
                serde_json::to_vec(&record.event)?,
            ));
            if batch.len() == BATCH {
                stage(database, conversation, std::mem::take(&mut batch))?;
            }
        }
    }
    stage(database, conversation, batch)?;
    let reserved: u64 = match std::fs::read(directory.session_sequence()) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let mut cursor: Option<Legacy> = None;
    let mut sequence = 0u64;
    loop {
        let rows = database
            .read_now(|db| {
                let mut query = db.prepare_cached(
                    "SELECT identity,sequenced,sequence,seconds,nanos,body FROM session_legacy
                WHERE conversation=?1 AND (sequenced,sequence,seconds,nanos,identity)>(?3,?4,?5,?6,?2)
                ORDER BY sequenced,sequence,seconds,nanos,identity LIMIT 16",
                )?;
                Ok(query
                    .query_map(
                        params![
                            conversation,
                            cursor.as_ref().map_or("", |row| row.0.as_str()),
                            cursor.as_ref().is_some_and(|row| row.1),
                            cursor.as_ref().map_or(&[0u8; 8][..], |row| row.2.as_slice()),
                            cursor.as_ref().map_or(i64::MIN, |row| row.3),
                            cursor.as_ref().map_or(i32::MIN, |row| row.4)
                        ],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, bool>(1)?,
                                row.get::<_, Vec<u8>>(2)?,
                                row.get::<_, i64>(3)?,
                                row.get::<_, i32>(4)?,
                                row.get::<_, Vec<u8>>(5)?,
                            ))
                        },
                    )?
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            })
            .map_err(io::Error::other)?;
        if rows.is_empty() {
            break;
        }
        cursor = rows.last().cloned();
        let tags = rows
            .iter()
            .map(|row| {
                serde_json::from_slice::<fastctx_types::session::SessionEvent>(&row.5)
                    .map(|event| super::search::Tags::of(&event))
                    .map_err(io::Error::other)
            })
            .collect::<io::Result<Vec<_>>>()?;
        let bytes = rows.iter().map(|row| row.5.len() + 128).sum();
        let scope = conversation.to_string();
        sequence = database
            .write_now_at(Priority::Background, bytes, move |db| {
                for ((key, sequenced, old, _, _, body), tags) in rows.into_iter().zip(tags) {
                    let next = sequence
                        .checked_add(1)
                        .ok_or_else(|| fastctx_storage::Error::new("session event sequence exhausted"))?;
                    sequence = if sequenced { next.max(sql::number(old)?) } else { next };
                    let existing: Option<Vec<u8>> = db
                        .query_row(
                            "SELECT body FROM session_events WHERE conversation=?1 AND identity=?2",
                            (&scope, &key),
                            |row| row.get(0),
                        )
                        .optional()?;
                    if let Some(existing) = existing {
                        if existing != body {
                            return Err(fastctx_storage::Error::new("a migrated session event changed"));
                        }
                    } else {
                        db.execute(
                            "INSERT INTO session_events(conversation,sequence,identity,body) VALUES(?1,?2,?3,?4)",
                            params![scope, sequence.to_be_bytes().as_slice(), key, body],
                        )?;
                    }
                    tags.insert(db, &scope, sequence)?;
                }
                Ok(sequence)
            })
            .map_err(io::Error::other)?;
    }
    database.read_now(|db| {
        let mismatch:i64=db.query_row("SELECT count(*) FROM session_legacy AS old LEFT JOIN session_events AS new ON old.conversation=new.conversation AND old.identity=new.identity
            WHERE old.conversation=?1 AND (new.body IS NULL OR new.body!=old.body)",[conversation],|row|row.get(0))?;
        if mismatch!=0 {return Err(fastctx_storage::Error::new("session event migration did not verify"))}
        Ok(())
    }).map_err(io::Error::other)?;
    let scope = conversation.to_string();
    database
        .write_now_at(Priority::Background, 128, move |db| {
            super::search::advance(db, &scope, sequence)?;
            sql::set_head(db, &scope, reserved.max(sequence), sequence, true)
        })
        .map_err(io::Error::other)?;
    Ok(())
}

fn stage(database: &Database, conversation: &str, rows: Vec<Legacy>) -> io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let bytes = rows.iter().map(|row| row.5.len() + 128).sum();
    let scope = conversation.to_string();
    database.write_now_at(Priority::Background,bytes,move |db| {
        let mut insert=db.prepare_cached("INSERT INTO session_legacy(conversation,identity,sequenced,sequence,seconds,nanos,body) VALUES(?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(conversation,identity) DO UPDATE SET sequenced=excluded.sequenced,sequence=excluded.sequence,seconds=excluded.seconds,nanos=excluded.nanos,body=excluded.body
            WHERE body!=excluded.body OR sequence!=excluded.sequence")?;
        for (key,sequenced,sequence,seconds,nanos,body) in rows {insert.execute(params![scope,key,sequenced,sequence,seconds,nanos,body])?;}
        Ok(())
    }).map_err(io::Error::other)
}
