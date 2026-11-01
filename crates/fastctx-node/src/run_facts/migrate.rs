//! Legacy call files are independent sources. Snapshots precede journals; each
//! committed file is reread and checked before its original is archived.
use super::{snapshot::StoredCall, sql};
use crate::record_migration::{self as migration, Context, scheduler};
use fastctx_platform::disk;
use fastctx_storage::{Database, rusqlite::OptionalExtension};
use fastctx_types::{layout::ConversationDir, run_facts::JobFacts};
use std::{
    io::{self, BufRead, Seek},
    path::Path,
};

fn imported(database: &Database, conversation: &str) -> io::Result<bool> {
    database
        .read_background_now(|db| {
            Ok(crate::conversation_store::deleted(db, conversation)?
                || db
                    .query_row("SELECT checked FROM run_imported WHERE conversation=?1", [conversation], |row| {
                        row.get::<_, bool>(0)
                    })
                    .optional()?
                    .unwrap_or(false))
        })
        .map_err(io::Error::other)
}
pub(super) fn ensure(directory: &ConversationDir, database: &Database, conversation: &str) -> io::Result<()> {
    let world = migration::world(directory)?;
    if fastctx_platform::roles::current().may_block() && imported(database, conversation)? {
        scheduler::ready(&world, conversation, "run-facts")?;
        return Ok(());
    }
    let context = Context {
        database: database.clone(),
        world: world.clone(),
        conversation: conversation.into(),
        family: "run-facts".into(),
    };
    let directory = directory.clone();
    scheduler::request(&world, conversation, "run-facts", move || import(&directory, &context))
}
fn import(directory: &ConversationDir, context: &Context) -> io::Result<()> {
    if imported(&context.database, &context.conversation)? {
        return Ok(());
    }
    let files = entries(&directory.calls())?;
    for path in files.iter().filter(|path| path.extension().is_some_and(|ext| ext == "json")) {
        migration::source::process(context, path, || snapshot(context, path), || verify_snapshot(context, path))?;
    }
    // Discover sidecars independently: a verified snapshot may already have
    // moved to the archive when the node stops before processing its journal.
    for path in &files {
        if path.extension().is_some_and(|ext| ext == "updates") {
            migration::source::process(context, path, || journal(context, path), || Ok(()))?;
        } else if path.extension().is_some_and(|ext| ext == "jobs") && disk::symlink_metadata(path)?.is_dir() {
            for job in entries(path)? {
                if job.extension().is_some_and(|ext| ext == "json") {
                    migration::source::process(
                        context,
                        &job,
                        || job_snapshot(context, &job),
                        || verify_job(context, &job),
                    )?;
                }
            }
        }
    }
    let scope = context.conversation.clone();
    context.database.write_now_at(scheduler::priority(), 64, move |db| { db.execute("INSERT INTO run_imported(conversation,checked) VALUES(?1,1) ON CONFLICT(conversation) DO UPDATE SET checked=1", [scope])?; Ok(()) }).map_err(io::Error::other)
}
fn entries(path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    match disk::read_dir(path) {
        Ok(entries) => {
            let mut paths = entries.map(|entry| entry.map(|entry| entry.path())).collect::<io::Result<Vec<_>>>()?;
            paths.sort();
            Ok(paths)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
fn snapshot(context: &Context, path: &Path) -> io::Result<()> {
    let bytes = disk::read(path)?;
    let mut stored: StoredCall = serde_json::from_slice(&bytes)?;
    if path.file_stem().and_then(|name| name.to_str())
        != Some(fastctx_hands::files::content_hash(stored.call_id.as_bytes()).as_str())
    {
        return Err(invalid("call identity differs from its legacy filename"));
    }
    let (writes, steps, visits) = (
        std::mem::take(&mut stored.facts.writes),
        std::mem::take(&mut stored.facts.steps),
        std::mem::take(&mut stored.facts.visited),
    );
    let projections =
        super::projections::Call::prepare(&context.database, &context.conversation, &mut stored, scheduler::priority())
            .map_err(io::Error::other)?;
    let call = stored.call_id.clone();
    let scope = context.conversation.clone();
    context
        .database
        .write_now_at(scheduler::priority(), bytes.len(), move |db| {
            if sql::head(db, &scope, &stored.call_id)?.is_none() {
                sql::save(db, &scope, &stored)?;
                projections.install(db, &scope, &stored.call_id, false)?;
            } else {
                projections.finish(db, &scope)?;
            }
            Ok(())
        })
        .map_err(io::Error::other)?;
    for index in 0..writes.len().max(steps.len()).max(visits.len()).div_ceil(16) {
        scheduler::checkpoint()?;
        let writes = writes.iter().skip(index * 16).take(16).cloned().collect::<Vec<_>>();
        let steps = steps.iter().skip(index * 16).take(16).cloned().collect::<Vec<_>>();
        let visits = visits.iter().skip(index * 16).take(16).cloned().collect::<Vec<_>>();
        let (scope, call) = (context.conversation.clone(), call.clone());
        let bytes = serde_json::to_vec(&(&writes, &steps, &visits))?.len();
        context
            .database
            .write_now_at(scheduler::priority(), bytes, move |db| {
                sql::writes(db, &scope, &call, &writes)?;
                sql::steps(db, &scope, &call, &steps)?;
                sql::visits(db, &scope, &call, &visits)
            })
            .map_err(io::Error::other)?;
    }
    Ok(())
}
fn verify_snapshot(context: &Context, path: &Path) -> io::Result<()> {
    let mut original: StoredCall = serde_json::from_reader(disk::open(path)?)?;
    verify_members(context, &original.call_id, &original.facts.writes, &original.facts.steps, &original.facts.visited)?;
    let mut stored = context
        .database
        .read_background_now(|db| sql::load(db, &context.conversation, &original.call_id))
        .map_err(io::Error::other)?
        .ok_or_else(|| invalid("imported call is absent"))?;
    for row in [&mut stored, &mut original] {
        row.facts.writes.clear();
        row.facts.steps.clear();
        row.facts.visited.clear();
        row.facts.folded_steps.clear();
    }
    if serde_json::to_vec(&stored)? != serde_json::to_vec(&original)? {
        return Err(invalid("call snapshot differs from reread original"));
    }
    Ok(())
}
fn verify_members(
    context: &Context,
    call: &str,
    writes: &[fastctx_types::run_facts::WriteFact],
    steps: &[fastctx_types::run_facts::RunStep],
    visits: &[(fastctx_types::MachineName, fastctx_types::JobId)],
) -> io::Result<()> {
    for index in 0..writes.len().max(steps.len()).max(visits.len()).div_ceil(64) {
        scheduler::checkpoint()?;
        let chunk = |length: usize| (index * 64).min(length)..((index + 1) * 64).min(length);
        let delta = context
            .database
            .read_background_now(|db| {
                sql::difference(
                    db,
                    &context.conversation,
                    call,
                    &writes[chunk(writes.len())],
                    &steps[chunk(steps.len())],
                    &visits[chunk(visits.len())],
                )
            })
            .map_err(io::Error::other)?;
        if !delta.writes.is_empty() || !delta.steps.is_empty() || !delta.visited.is_empty() {
            return Err(invalid("call members differ from reread original"));
        }
    }
    Ok(())
}
fn journal(context: &Context, path: &Path) -> io::Result<()> {
    let key = path.to_string_lossy().into_owned();
    let signature = migration::archive::fingerprint(path)?;
    let prior = context
        .database
        .read_background_now(|db| {
            Ok(db
                .query_row(
                    "SELECT signature,through FROM migration_files WHERE family=?1 AND path=?2",
                    (&context.family, &key),
                    |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()?)
        })
        .map_err(io::Error::other)?;
    let mut offset = if let Some((old, through)) = prior {
        if serde_json::from_slice::<migration::archive::Signature>(&old)? != signature {
            return Err(invalid("call journal changed after a committed frame"));
        }
        through as u64
    } else {
        0
    };
    let mut file = disk::open(path)?;
    file.seek(io::SeekFrom::Start(offset))?;
    let mut reader = io::BufReader::new(file);
    let mut verifier = io::BufReader::new(disk::open(path)?);
    let mut line = Vec::new();
    let mut reread = Vec::new();
    loop {
        scheduler::checkpoint()?;
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") {
            return Ok(());
        }
        let update: JobFacts = serde_json::from_slice(&line)?;
        if path.file_stem().and_then(|name| name.to_str())
            != Some(fastctx_hands::files::content_hash(update.call_id.as_bytes()).as_str())
        {
            return Err(invalid("call journal belongs to another call"));
        }
        let present = context
            .database
            .read_background_now(|db| Ok(sql::head(db, &context.conversation, &update.call_id)?.is_some()))
            .map_err(io::Error::other)?;
        if !present {
            return Err(invalid("call journal has no valid snapshot"));
        }
        super::acceptance::receive_at(
            &context.database,
            &context.conversation,
            &update,
            scheduler::priority(),
            |_| {},
        )?;
        verifier.seek(io::SeekFrom::Start(offset))?;
        reread.clear();
        verifier.read_until(b'\n', &mut reread)?;
        if reread != line {
            return Err(invalid("call journal frame changed before verification"));
        }
        let original: JobFacts = serde_json::from_slice(&reread)?;
        verify_members(context, &original.call_id, &original.writes, &original.steps, &original.visited)?;
        verify_job_fields(context, &original)?;
        offset += line.len() as u64;
        let (scope, family, path, proof) =
            (context.conversation.clone(), context.family.clone(), key.clone(), serde_json::to_vec(&signature)?);
        context.database.write_now_at(scheduler::priority(), 256, move |db| {
            db.execute("INSERT INTO migration_files(conversation,family,path,signature,through) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(family,path) DO UPDATE SET through=excluded.through", (scope, family, path, proof, offset as i64))?;
            Ok(())
        }).map_err(io::Error::other)?;
    }
}
fn verify_job_fields(context: &Context, original: &JobFacts) -> io::Result<()> {
    let stored = context
        .database
        .read_background_now(|db| {
            let transaction = db.unchecked_transaction()?;
            let mut stored = sql::job(&transaction, &context.conversation, original)?;
            if let Some(stored) = &mut stored {
                sql::job_source(&transaction, &context.conversation, stored)?;
            }
            transaction.commit()?;
            Ok(stored)
        })
        .map_err(io::Error::other)?
        .ok_or_else(|| invalid("job snapshot is absent"))?;
    if stored.sequence < original.sequence {
        return Err(invalid("job snapshot is older than reread original"));
    }
    if stored.sequence == original.sequence {
        for (expected, actual) in [
            (serde_json::to_value(&original.state)?, serde_json::to_value(&stored.state)?),
            (serde_json::to_value(&original.output)?, serde_json::to_value(&stored.output)?),
            (serde_json::to_value(&original.notes)?, serde_json::to_value(&stored.notes)?),
            (serde_json::to_value(&original.turn)?, serde_json::to_value(&stored.turn)?),
            (serde_json::to_value(&original.record)?, serde_json::to_value(&stored.record)?),
            (serde_json::to_value(&original.script)?, serde_json::to_value(&stored.script)?),
            (serde_json::to_value(original.started_at)?, serde_json::to_value(stored.started_at)?),
            (serde_json::to_value(original.finished_at)?, serde_json::to_value(stored.finished_at)?),
        ] {
            if !expected.is_null() && expected != actual {
                return Err(invalid("job metadata differs from reread original"));
            }
        }
    }
    Ok(())
}
fn job_snapshot(context: &Context, path: &Path) -> io::Result<()> {
    let bytes = disk::read(path)?;
    let mut update: JobFacts = serde_json::from_slice(&bytes)?;
    let call = fastctx_hands::files::content_hash(update.call_id.as_bytes());
    let job = fastctx_hands::files::content_hash(format!("{}:{}", update.machine, update.job).as_bytes());
    if path.file_stem().and_then(|name| name.to_str()) != Some(job.as_str())
        || path.parent().and_then(Path::file_stem).and_then(|name| name.to_str()) != Some(call.as_str())
    {
        return Err(invalid("job identity differs from its legacy filename or parent call"));
    }
    let has_call = context
        .database
        .read_background_now(|db| Ok(sql::head(db, &context.conversation, &update.call_id)?.is_some()))
        .map_err(io::Error::other)?;
    if has_call {
        return super::acceptance::receive_at(
            &context.database,
            &context.conversation,
            &update,
            scheduler::priority(),
            |_| {},
        )
        .map(|_| ());
    }
    for index in 0..update.writes.len().max(update.steps.len()).max(update.visited.len()).div_ceil(16) {
        scheduler::checkpoint()?;
        let writes = update.writes.iter().skip(index * 16).take(16).cloned().collect::<Vec<_>>();
        let steps = update.steps.iter().skip(index * 16).take(16).cloned().collect::<Vec<_>>();
        let visits = update.visited.iter().skip(index * 16).take(16).cloned().collect::<Vec<_>>();
        let (scope, call) = (context.conversation.clone(), update.call_id.clone());
        let bytes = serde_json::to_vec(&(&writes, &steps, &visits))?.len();
        context
            .database
            .write_now_at(scheduler::priority(), bytes, move |db| {
                sql::writes(db, &scope, &call, &writes)?;
                sql::steps(db, &scope, &call, &steps)?;
                sql::visits(db, &scope, &call, &visits)
            })
            .map_err(io::Error::other)?;
    }
    let notes = update
        .notes
        .take()
        .map(|notes| {
            crate::record_payloads::stage(
                &context.database,
                &context.conversation,
                sql::encode(&notes)?,
                scheduler::priority(),
            )
        })
        .transpose()
        .map_err(io::Error::other)?;
    let scope = context.conversation.clone();
    context
        .database
        .write_now_at(scheduler::priority(), bytes.len(), move |db| {
            if sql::job(db, &scope, &update)?.is_none() {
                sql::save_job(db, &scope, &update, notes.as_ref().map(|notes| &notes.reference))?;
            }
            if let Some(notes) = &notes {
                crate::record_payloads::finish(db, &scope, notes)?;
            }
            Ok(())
        })
        .map_err(io::Error::other)
}
fn verify_job(context: &Context, path: &Path) -> io::Result<()> {
    let original: JobFacts = serde_json::from_reader(disk::open(path)?)?;
    verify_members(context, &original.call_id, &original.writes, &original.steps, &original.visited)?;
    verify_job_fields(context, &original)
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
