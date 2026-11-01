//! Legacy sequence numbers never change. Unnumbered events precede them when
//! enough positive numbers are free; otherwise their old relative order follows
//! the reserved high-water mark. No caller executes the migration itself.
use super::{legacy, sql};
use crate::record_migration::{archive, scheduler};
use fastctx_platform::disk;
use fastctx_storage::{
    Database,
    rusqlite::{OptionalExtension, params},
};
use fastctx_types::layout::ConversationDir;
use std::io;

pub(super) fn ensure(directory: &ConversationDir, database: &Database, conversation: &str) -> io::Result<()> {
    let world = crate::record_migration::world(directory)?;
    if fastctx_platform::roles::current().may_block()
        && database
            .read_background_now(|db| {
                Ok(crate::conversation_store::deleted(db, conversation)? || sql::head(db, conversation)?.2)
            })
            .map_err(io::Error::other)?
    {
        scheduler::ready(&world, conversation, "session-events");
        return Ok(());
    }
    let (directory, database, scope) = (directory.clone(), database.clone(), conversation.to_string());
    scheduler::request(&world, conversation, "session-events", move || import(&directory, &database, &scope))
}
fn import(directory: &ConversationDir, database: &Database, conversation: &str) -> io::Result<()> {
    let (deleted, imported) = database
        .read_background_now(|db| {
            Ok((crate::conversation_store::deleted(db, conversation)?, sql::head(db, conversation)?.2))
        })
        .map_err(io::Error::other)?;
    if deleted {
        return Ok(());
    }
    if imported {
        return legacy::cleanup(database, conversation);
    }
    legacy::scan(directory, database, conversation)?;
    legacy::assign(directory, database, conversation)?;
    let world = crate::record_migration::world(directory)?;
    loop {
        scheduler::checkpoint()?;
        let batch = legacy::batch(database, conversation)?;
        if batch.is_empty() {
            break;
        }
        let mut rows = Vec::new();
        let mut finished = Vec::new();
        for (source, sequence) in batch {
            let path = match disk::metadata(&source.path) {
                Ok(_) => source.path.clone(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    archive::verified_path(&world, conversation, &source.path, &source.signature)?
                }
                Err(error) => return Err(error),
            };
            match legacy::read(&path) {
                Ok((record, signature))
                    if signature == source.signature && record.sequence == source.sequence && sequence != 0 =>
                {
                    let body = serde_json::to_vec(&record.event)?;
                    let tags = super::search::Tags::of(&record.event);
                    rows.push((source, sequence, body, tags));
                }
                Ok(_) => {
                    archive::damaged(
                        &world,
                        conversation,
                        "session-events",
                        &source.path,
                        "source or sequence differs from its staged metadata",
                    )?;
                    finished.push(source.identity);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Err(error),
                Err(error) => {
                    archive::damaged(&world, conversation, "session-events", &source.path, &error.to_string())?;
                    finished.push(source.identity);
                }
            }
        }
        let scope = conversation.to_string();
        let bytes = rows.iter().map(|(_, _, body, _)| body.len() + 128).sum();
        let results = database.write_now_at(scheduler::priority(), bytes, move |db| {
            let mut results = Vec::new();
            let mut lookup = db.prepare_cached("SELECT sequence,identity,body FROM session_events WHERE conversation=?1 AND identity=?2
                UNION ALL SELECT sequence,identity,body FROM session_events WHERE conversation=?1 AND sequence=?3 LIMIT 1")?;
            for (source, sequence, body, tags) in rows {
                let existing: Option<(Vec<u8>, String, Vec<u8>)> = lookup.query_row(
                    params![scope, source.identity, sequence.to_be_bytes().as_slice()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
                let conflict = existing.as_ref().is_some_and(|(old,identity,stored)| old.as_slice() != sequence.to_be_bytes() || identity != &source.identity || stored != &body);
                if !conflict {
                    if existing.is_none() { db.execute("INSERT INTO session_events(conversation,sequence,identity,body) VALUES(?1,?2,?3,?4)", params![scope, sequence.to_be_bytes().as_slice(), source.identity, body])?; }
                    tags.insert(db, &scope, sequence)?;
                }
                results.push((source, sequence, conflict));
            }
            Ok(results)
        }).map_err(io::Error::other)?;
        let mut verified = Vec::new();
        for (source, sequence, conflict) in results {
            if conflict {
                archive::damaged(
                    &world,
                    conversation,
                    "session-events",
                    &source.path,
                    "event identity or original sequence conflicts with a different stored event",
                )?;
                finished.push(source.identity);
                continue;
            }
            let original = match legacy::read(&source.path) {
                Ok(value) => value,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    legacy::read(&archive::verified_path(&world, conversation, &source.path, &source.signature)?)?
                }
                Err(error) => return Err(error),
            };
            if original.1 != source.signature {
                archive::damaged(
                    &world,
                    conversation,
                    "session-events",
                    &source.path,
                    "session source changed before verification",
                )?;
                finished.push(source.identity);
                continue;
            }
            verified.push((source, sequence, serde_json::to_vec(&original.0.event)?));
        }
        if !verified.is_empty() {
            let matches = database
                .read_background_now(|db| {
                    let transaction = db.unchecked_transaction()?;
                    let mut query = transaction.prepare_cached(
                        "SELECT sequence=?3 AND body=?4 FROM session_events WHERE conversation=?1 AND identity=?2",
                    )?;
                    for (source, sequence, body) in &verified {
                        if !query.query_row(
                            params![conversation, source.identity, sequence.to_be_bytes().as_slice(), body],
                            |row| row.get::<_, bool>(0),
                        )? {
                            return Ok(false);
                        }
                    }
                    drop(query);
                    transaction.commit()?;
                    Ok(true)
                })
                .map_err(io::Error::other)?;
            if !matches {
                return Err(io::Error::other("session event differs from the reread original"));
            }
            crate::conversation_store::with_live_files(directory, || {
                for (source, _, _) in &verified {
                    archive::verified(&world, conversation, &source.path, &source.signature)?;
                }
                Ok(())
            })?;
            finished.extend(verified.into_iter().map(|(source, _, _)| source.identity));
        }
        legacy::finish(database, conversation, finished)?;
    }
    let sequence = directory.session_sequence();
    match archive::fingerprint(&sequence) {
        Ok(signature) => crate::conversation_store::with_live_files(directory, || {
            archive::verified(&world, conversation, &sequence, &signature)
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let scope = conversation.to_string();
    database
        .write_now_at(scheduler::priority(), 128, move |db| {
            let latest: Option<Vec<u8>> =
                db.query_row("SELECT max(sequence) FROM session_events WHERE conversation=?1", [&scope], |row| {
                    row.get(0)
                })?;
            let latest = latest.map(sql::number).transpose()?.unwrap_or(0);
            let reserved = sql::number(db.query_row(
                "SELECT reserved FROM session_migration_state WHERE conversation=?1",
                [&scope],
                |row| row.get(0),
            )?)?;
            let prior = sql::head(db, &scope)?;
            super::search::advance(db, &scope, latest)?;
            sql::set_head(db, &scope, reserved.max(latest).max(prior.0), latest, true)
        })
        .map_err(io::Error::other)?;
    legacy::cleanup(database, conversation)
}
