//! A compact source index keeps ordering and restart checkpoints without keeping
//! a second copy of event bodies. The original remains the verification source.
use super::{Recorded, sql};
use crate::record_migration::{archive, scheduler};
use fastctx_platform::disk;
use fastctx_storage::{Database, rusqlite::params};
use fastctx_types::layout::ConversationDir;
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_sources (
    conversation TEXT NOT NULL, identity TEXT NOT NULL, path TEXT NOT NULL,
    signature BLOB NOT NULL, sequence BLOB, seconds INTEGER NOT NULL, nanos INTEGER NOT NULL,
    assigned BLOB, finished INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(conversation,identity)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS session_sources_order ON session_sources(conversation,finished,sequence,seconds,nanos,identity);
CREATE INDEX IF NOT EXISTS session_sources_unassigned ON session_sources(conversation,assigned,seconds,nanos,identity);
CREATE TABLE IF NOT EXISTS session_migration_state (
    conversation TEXT PRIMARY KEY, base BLOB NOT NULL, next BLOB NOT NULL, reserved BLOB NOT NULL
) WITHOUT ROWID;
";

#[derive(Clone)]
pub(super) struct Source {
    pub identity: String,
    pub path: PathBuf,
    pub signature: archive::Signature,
    pub sequence: Option<u64>,
    pub seconds: i64,
    pub nanos: i32,
}

pub(super) fn read(path: &Path) -> io::Result<(Recorded, archive::Signature)> {
    let bytes = disk::read(path)?;
    let record = serde_json::from_slice(&bytes)?;
    let signature = archive::Signature { bytes: bytes.len() as u64, digest: format!("{:x}", Sha256::digest(&bytes)) };
    Ok((record, signature))
}

pub(super) fn scan(directory: &ConversationDir, database: &Database, conversation: &str) -> io::Result<()> {
    let world = crate::record_migration::world(directory)?;
    let entries = match disk::read_dir(directory.session_events()) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let mut batch = Vec::new();
    for entry in entries.into_iter().flatten() {
        scheduler::checkpoint()?;
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json")
            || path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let Some(identity) = path.file_stem().and_then(|name| name.to_str()) else { continue };
        let known = database
            .read_background_now(|db| {
                Ok(db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_sources WHERE conversation=?1 AND identity=?2)",
                    (conversation, identity),
                    |row| row.get::<_, bool>(0),
                )?)
            })
            .map_err(io::Error::other)?;
        if known {
            continue;
        }
        match read(&path) {
            Ok((record, signature)) => batch.push(Source {
                identity: identity.into(),
                path,
                signature,
                sequence: record.sequence,
                seconds: record.event.at.as_second(),
                nanos: record.event.at.subsec_nanosecond(),
            }),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Err(error),
            Err(error) => archive::damaged(&world, conversation, "session-events", &path, &error.to_string())?,
        }
        if batch.len() == 128 {
            stage(database, conversation, std::mem::take(&mut batch))?;
        }
    }
    stage(database, conversation, batch)
}
fn stage(database: &Database, conversation: &str, sources: Vec<Source>) -> io::Result<()> {
    if sources.is_empty() {
        return Ok(());
    }
    let conversation = conversation.to_string();
    let sources = sources
        .into_iter()
        .map(|source| Ok((serde_json::to_vec(&source.signature)?, source)))
        .collect::<io::Result<Vec<_>>>()?;
    database.write_now_at(scheduler::priority(), sources.len() * 256, move |db| {
        let mut insert = db.prepare_cached("INSERT INTO session_sources(conversation,identity,path,signature,sequence,seconds,nanos,assigned) VALUES(?1,?2,?3,?4,?5,?6,?7,?5) ON CONFLICT(conversation,identity) DO NOTHING")?;
        for (signature, source) in sources { insert.execute(params![conversation, source.identity, source.path.to_string_lossy(), signature,
            source.sequence.map(|n| n.to_be_bytes().to_vec()), source.seconds, source.nanos])?; }
        Ok(())
    }).map_err(io::Error::other)
}

pub(super) fn assign(directory: &ConversationDir, database: &Database, conversation: &str) -> io::Result<()> {
    let world = crate::record_migration::world(directory)?;
    let reserved = match disk::read(directory.session_sequence()) {
        Ok(bytes) => match serde_json::from_slice::<u64>(&bytes) {
            Ok(value) => value,
            Err(error) => {
                archive::damaged(
                    &world,
                    conversation,
                    "session-sequence",
                    &directory.session_sequence(),
                    &error.to_string(),
                )?;
                0
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let scope = conversation.to_string();
    database
        .write_now_at(scheduler::priority(), 256, move |db| {
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_migration_state WHERE conversation=?1)",
                [&scope],
                |row| row.get(0),
            )?;
            if exists {
                return Ok(());
            }
            let (first, last): (Option<Vec<u8>>, Option<Vec<u8>>) = db.query_row(
                "SELECT min(sequence),max(sequence) FROM session_sources WHERE conversation=?1",
                [&scope],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let first = first.map(sql::number).transpose()?;
            let last = last.map(sql::number).transpose()?.unwrap_or(0);
            let count: i64 = db.query_row(
                "SELECT count(*) FROM session_sources WHERE conversation=?1 AND sequence IS NULL",
                [&scope],
                |row| row.get(0),
            )?;
            let count = count as u64;
            let (allocated, latest, _) = sql::head(db, &scope)?;
            let base = if first.is_none_or(|first| first > count) && latest == 0 {
                0
            } else {
                last.max(allocated).max(reserved)
            };
            base.checked_add(count).ok_or_else(|| fastctx_storage::Error::new("session event sequence exhausted"))?;
            db.execute(
                "INSERT INTO session_migration_state(conversation,base,next,reserved) VALUES(?1,?2,?2,?3)",
                params![scope, base.to_be_bytes().as_slice(), reserved.to_be_bytes().as_slice()],
            )?;
            Ok(())
        })
        .map_err(io::Error::other)?;
    loop {
        scheduler::checkpoint()?;
        let scope = conversation.to_string();
        let more = database.write_now_at(scheduler::priority(), 128 * 64, move |db| {
            let mut next = sql::number(db.query_row("SELECT next FROM session_migration_state WHERE conversation=?1", [&scope], |row| row.get(0))?)?;
            let keys = db.prepare_cached("SELECT identity FROM session_sources WHERE conversation=?1 AND sequence IS NULL AND assigned IS NULL ORDER BY seconds,nanos,identity LIMIT 128")?
                .query_map([&scope], |row| row.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
            let mut update = db.prepare_cached("UPDATE session_sources SET assigned=?3 WHERE conversation=?1 AND identity=?2")?;
            for key in &keys { next = next.checked_add(1).ok_or_else(|| fastctx_storage::Error::new("session sequence exhausted"))?;
                update.execute(params![scope, key, next.to_be_bytes().as_slice()])?; }
            db.execute("UPDATE session_migration_state SET next=?2 WHERE conversation=?1", params![scope, next.to_be_bytes().as_slice()])?;
            Ok(!keys.is_empty())
        }).map_err(io::Error::other)?;
        if !more {
            return Ok(());
        }
    }
}

pub(super) fn batch(database: &Database, conversation: &str) -> io::Result<Vec<(Source, u64)>> {
    database.read_background_now(|db| {
        let rows = db.prepare_cached("SELECT identity,path,signature,sequence,seconds,nanos,assigned FROM session_sources WHERE conversation=?1 AND finished=0 ORDER BY sequence,seconds,nanos,identity LIMIT 16")?
            .query_map([conversation], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?, row.get::<_, Option<Vec<u8>>>(3)?, row.get::<_, i64>(4)?, row.get::<_, i32>(5)?, row.get::<_, Vec<u8>>(6)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter().map(|(identity,path,signature,sequence,seconds,nanos,assigned)| Ok((Source { identity, path: path.into(),
            signature: serde_json::from_slice(&signature).map_err(|error| fastctx_storage::Error::new(error.to_string()))?, sequence: sequence.map(sql::number).transpose()?, seconds,nanos }, sql::number(assigned)?))).collect()
    }).map_err(io::Error::other)
}

pub(super) fn finish(database: &Database, conversation: &str, keys: Vec<String>) -> io::Result<()> {
    let scope = conversation.to_string();
    database
        .write_now_at(scheduler::priority(), keys.len() * 64, move |db| {
            let mut update =
                db.prepare_cached("UPDATE session_sources SET finished=1 WHERE conversation=?1 AND identity=?2")?;
            for key in keys {
                update.execute((&scope, key))?;
            }
            Ok(())
        })
        .map_err(io::Error::other)
}

pub(super) fn cleanup(database: &Database, conversation: &str) -> io::Result<()> {
    loop {
        scheduler::checkpoint()?;
        let scope = conversation.to_string();
        let removed = database.write_now_at(scheduler::priority(), 128 * 64, move |db| Ok(db.execute(
            "DELETE FROM session_sources WHERE conversation=?1 AND identity IN (SELECT identity FROM session_sources WHERE conversation=?1 LIMIT 128)", [&scope])?)).map_err(io::Error::other)?;
        if removed == 0 {
            break;
        }
    }
    let scope = conversation.to_string();
    database
        .write_now_at(scheduler::priority(), 64, move |db| {
            db.execute("DELETE FROM session_migration_state WHERE conversation=?1", [scope])?;
            Ok(())
        })
        .map_err(io::Error::other)
}
