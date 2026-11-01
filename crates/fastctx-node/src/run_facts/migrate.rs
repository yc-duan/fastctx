//! Resumable import of legacy call snapshots and incremental journals.

use super::{snapshot::StoredCall, sql};
use fastctx_storage::{Database, Priority, rusqlite::OptionalExtension};
use fastctx_types::{layout::ConversationDir, run_facts::JobFacts};
use std::{io, path::Path, sync::LazyLock};

static MIGRATING: LazyLock<crate::record_gates::Gates> =
    LazyLock::new(|| crate::record_gates::Gates::new(fastctx_platform::doing::Rank::RECORDS, "migrating call facts"));

fn imported(database: &Database, conversation: &str) -> io::Result<bool> {
    database
        .read_now(|db| {
            Ok(db
                .query_row("SELECT checked FROM run_imported WHERE conversation=?1", [conversation], |row| {
                    row.get::<_, bool>(0)
                })
                .optional()?
                .unwrap_or(false))
        })
        .map_err(io::Error::other)
}

pub(super) fn ensure(directory: &ConversationDir, database: &Database, conversation: &str) -> io::Result<()> {
    if database.read_now(|db| crate::conversation_store::deleted(db, conversation)).map_err(io::Error::other)?
        || imported(database, conversation)?
    {
        return Ok(());
    }
    let gate = MIGRATING.get(directory.path());
    let _gate = gate.lock();
    if database.read_now(|db| crate::conversation_store::deleted(db, conversation)).map_err(io::Error::other)?
        || imported(database, conversation)?
    {
        return Ok(());
    }
    let entries = match std::fs::read_dir(directory.calls()) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(entries) = entries {
        let mut calls = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|extension| extension == "json") {
                calls.push(path);
            }
        }
        calls.sort();
        for path in calls {
            import(database, conversation, &path)?;
        }
    }
    let conversation = conversation.to_string();
    database.write_now_at(Priority::Background,conversation.len(),move |db| {
        db.execute("INSERT INTO run_imported(conversation,checked) VALUES(?1,1) ON CONFLICT(conversation) DO UPDATE SET checked=1",[conversation])?;
        Ok(())
    }).map_err(io::Error::other)
}

fn import(database: &Database, conversation: &str, path: &Path) -> io::Result<()> {
    let bytes = std::fs::read(path)?;
    let mut stored: StoredCall = serde_json::from_slice(&bytes)?;
    let (writes, steps, visits) =
        (stored.facts.writes.clone(), stored.facts.steps.clone(), stored.facts.visited.clone());
    let projections = super::projections::Call::prepare(database, conversation, &mut stored, Priority::Background)
        .map_err(io::Error::other)?;
    let call = stored.call_id.clone();
    let scope = conversation.to_string();
    database
        .write_now_at(Priority::Background, bytes.len(), move |db| {
            if sql::head(db, &scope, &stored.call_id)?.is_none() {
                sql::save(db, &scope, &stored)?;
                projections.install(db, &scope, &stored.call_id, false)?;
            } else {
                projections.finish(db, &scope)?;
            }
            Ok(())
        })
        .map_err(io::Error::other)?;
    for chunk in writes.chunks(16) {
        let (scope, call, chunk) = (conversation.to_string(), call.clone(), chunk.to_vec());
        let bytes = sql::encode(&chunk).map_err(io::Error::other)?.len();
        database
            .write_now_at(Priority::Background, bytes, move |db| sql::writes(db, &scope, &call, &chunk))
            .map_err(io::Error::other)?;
    }
    for chunk in steps.chunks(16) {
        let (scope, call, chunk) = (conversation.to_string(), call.clone(), chunk.to_vec());
        let bytes = sql::encode(&chunk).map_err(io::Error::other)?.len();
        database
            .write_now_at(Priority::Background, bytes, move |db| sql::steps(db, &scope, &call, &chunk))
            .map_err(io::Error::other)?;
    }
    for chunk in visits.chunks(16) {
        let (scope, call, chunk) = (conversation.to_string(), call.clone(), chunk.to_vec());
        database
            .write_now_at(Priority::Background, chunk.len() * 128, move |db| sql::visits(db, &scope, &call, &chunk))
            .map_err(io::Error::other)?;
    }
    super::log::read(&path.with_extension("updates"), |row| {
        let update: JobFacts = serde_json::from_slice(row)?;
        super::acceptance::receive_at(database, conversation, &update, Priority::Background, |_| {})?;
        Ok(())
    })?;
    let jobs = path.with_extension("jobs");
    match std::fs::read_dir(jobs) {
        Ok(entries) => {
            for entry in entries {
                let path = entry?.path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let bytes = std::fs::read(path)?;
                let mut update: JobFacts = serde_json::from_slice(&bytes)?;
                let notes = update
                    .notes
                    .take()
                    .map(|notes| {
                        crate::record_payloads::stage(
                            database,
                            conversation,
                            sql::encode(&notes)?,
                            Priority::Background,
                        )
                    })
                    .transpose()
                    .map_err(io::Error::other)?;
                let scope = conversation.to_string();
                database
                    .write_now_at(Priority::Background, bytes.len(), move |db| {
                        if sql::job(db, &scope, &update)?.is_none() {
                            sql::save_job(db, &scope, &update, notes.as_ref().map(|notes| &notes.reference))?;
                        }
                        if let Some(notes) = &notes {
                            crate::record_payloads::finish(db, &scope, notes)?;
                        }
                        Ok(())
                    })
                    .map_err(io::Error::other)?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(())
}
