//! Request paths enqueue owned decoders; only migration workers import records.
mod occurrences;
pub(crate) mod proofs;
mod source;
use super::*;
use crate::record_migration::{self as migration, files::Context, scheduler};
use source::LegacyRow;
pub(crate) use source::Source;

pub(crate) fn ensure_each(
    directory: &ConversationDir,
    family: &str,
    source: impl FnOnce(&mut dyn FnMut(Source) -> io::Result<()>) -> io::Result<()> + Send + 'static,
) -> io::Result<()> {
    let (database, conversation) = locate(directory)?;
    let world = migration::world(directory)?;
    let context =
        Context { database, world: world.clone(), conversation: conversation.clone(), family: family.to_string() };
    if scheduler::known(&world, &conversation, family) {
        return Ok(());
    }
    if imported(&context)? {
        scheduler::ready(&world, &conversation, family);
        return Ok(());
    }
    scheduler::request(&world, &conversation, family, move || context.within(|| import(&context, source)))
}

/// A derived SQL migration has no legacy file to archive. Its operation must be
/// idempotent; completion is marked only after all its bounded commits succeed.
pub(crate) fn ensure_step(
    directory: &ConversationDir,
    family: &str,
    work: impl FnOnce() -> io::Result<()> + Send + 'static,
) -> io::Result<()> {
    ensure_each(directory, family, move |_| work())
}

pub(crate) fn ensure_file(
    directory: &ConversationDir,
    family: &str,
    path: std::path::PathBuf,
    mut source: impl FnMut(&std::path::Path, &mut dyn FnMut(Source) -> io::Result<()>) -> io::Result<()> + Send + 'static,
) -> io::Result<()> {
    ensure_each(directory, family, move |push| migration::file(&path, push, |push| source(&path, push)))
}

fn imported(context: &Context) -> io::Result<bool> {
    context
        .database
        .read_background_now(|db| {
            Ok(crate::conversation_store::deleted(db, &context.conversation)?
                || db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM conversation_record_imports WHERE conversation=?1 AND family=?2)",
                    (&context.conversation, &context.family),
                    |row| row.get(0),
                )?)
        })
        .map_err(io::Error::other)
}

fn import(
    context: &Context,
    source: impl FnOnce(&mut dyn FnMut(Source) -> io::Result<()>) -> io::Result<()>,
) -> io::Result<()> {
    if imported(context)? {
        return Ok(());
    }
    let mut pending = Vec::new();
    let mut checking = Vec::new();
    let mut bytes = 0;
    // What it brings over is committed relaxed, and reaches the disk before
    // an original is archived (`archive_ready`) and before the family is
    // marked done.
    let mut wrote = false;
    source(&mut |source| {
        scheduler::checkpoint()?;
        match source {
            Source::Row(row) => {
                if row.file.is_none() {
                    return Err(io::Error::other("a legacy decoder must declare the file it reads"));
                }
                bytes += proofs::bytes(&row);
                pending.push(row);
                wrote = true;
                if pending.len() >= 16 || bytes >= 128 * 1024 {
                    commit(context, std::mem::take(&mut pending))?;
                    bytes = 0;
                }
                Ok(())
            }
            Source::Flush => {
                commit(context, std::mem::take(&mut pending))?;
                bytes = 0;
                Ok(())
            }
            Source::Verify(row) => {
                bytes += proofs::bytes(&row);
                checking.push(row);
                if checking.len() >= 16 || bytes >= 128 * 1024 {
                    proofs::verify(context, std::mem::take(&mut checking))?;
                    bytes = 0;
                }
                Ok(())
            }
            Source::File(file) => {
                commit(context, std::mem::take(&mut pending))?;
                proofs::verify(context, std::mem::take(&mut checking))?;
                bytes = 0;
                if context
                    .database
                    .read_background_now(|db| crate::conversation_store::deleted(db, &file.owner))
                    .map_err(io::Error::other)?
                {
                    match fastctx_platform::disk::remove_file(&file.row.path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                    return Ok(());
                }
                wrote = true;
                migration::files::complete(context, file)
            }
        }
    })?;
    commit(context, pending)?;
    proofs::verify(context, checking)?;
    migration::files::archive_ready(context)?;
    let (conversation, family) = (context.conversation.clone(), context.family.clone());
    let done = move |db: &Connection| {
        db.execute(
            "INSERT OR IGNORE INTO conversation_record_imports(conversation,family) VALUES(?1,?2)",
            (conversation, family),
        )?;
        Ok(())
    };
    // A durable commit takes every relaxed one before it to the disk; a
    // family that brought nothing over is found done again if this is lost.
    if wrote {
        context.database.write_now_at(scheduler::priority(), 128, done)
    } else {
        context.database.write_relaxed_now_at(scheduler::priority(), 128, done)
    }
    .map_err(io::Error::other)
}

fn commit(context: &Context, sources: Vec<LegacyRow>) -> io::Result<()> {
    if sources.is_empty() {
        return Ok(());
    }
    let bytes = sources.iter().map(proofs::bytes).sum();
    let (conversation, family) = (context.conversation.clone(), context.family.clone());
    let conflict = context.database.write_relaxed_now_at(scheduler::priority(), bytes, move |db| {
        db.execute_batch("SAVEPOINT legacy_record_batch")?;
        let mut insert = db.prepare_cached("INSERT INTO conversation_records(conversation,family,identity,body,modified,arrival,category,expires,position) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?10)
            ON CONFLICT(conversation,family,identity) DO UPDATE SET body=excluded.body,modified=excluded.modified,arrival=excluded.arrival,category=excluded.category,expires=excluded.expires WHERE ?9 AND conversation_records.position>COALESCE((SELECT boundary FROM conversation_record_gathering WHERE conversation=?1),-1)")?;
        let mut read = db.prepare_cached("SELECT body,modified,arrival,category,expires FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3")?;
        let mut files = std::collections::HashMap::new();
        for mut row in sources {
            let owner = row.conversation.as_deref().unwrap_or(&conversation);
            if crate::conversation_store::deleted(db, owner)? { continue; }
            let owner=owner.to_string();
            let source_digest=proofs::digest(&row)?;
            occurrences::assign(db, &owner, &family, &mut row)?;
            let owner=owner.as_str();
            let repeated = if let Some((hash, deduplicate)) = &row.content_key {
                let inserted = db.execute("INSERT OR IGNORE INTO migration_content_keys(conversation,family,hash) VALUES(?1,?2,?3)", (owner, &family, hash))?;
                *deduplicate && inserted == 0
            } else { false };
            if !row.discard && !repeated {
            let changed = insert.execute(params![owner, family, row.key, row.body, row.modified.to_string(), row.arrival, row.category,
                row.expires.map(|at| at.as_millisecond()), row.replace, super::positions::next(db, owner, "records")?])?;
            let stored = read.query_row((owner, &family, &row.key), |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<Vec<u8>>>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, Option<i64>>(4)?)))?;
            if stored.0 != row.body || (changed > 0 && (stored.1 != row.modified.to_string() || stored.2 != row.arrival || stored.3 != row.category || stored.4 != row.expires.map(|at| at.as_millisecond()))) { db.execute_batch("ROLLBACK TO legacy_record_batch; RELEASE legacy_record_batch")?; return Ok(Some(format!("legacy row {} conflicts with its stored value", row.key))); }
            }
            proofs::keep(db, owner, &family, &row, source_digest, repeated)?;
            if let Some(file) = row.file { files.insert(file.path.clone(), (owner.to_string(), file)); }
        }
        for (owner, file) in files.into_values() {
            db.execute("INSERT INTO migration_files(conversation,family,path,signature,through,incarnation) VALUES(?1,?2,?3,?4,?5,?6)
                ON CONFLICT(family,path) DO UPDATE SET conversation=excluded.conversation,through=max(through,excluded.through)",
                (owner, &family, file.path, encode(&file.signature)?, file.ordinal, file.incarnation))?;
        }
        db.execute_batch("RELEASE legacy_record_batch")?;
        Ok(None)
    }).map_err(io::Error::other)?;
    match conflict {
        Some(message) => Err(io::Error::new(io::ErrorKind::InvalidData, message)),
        None => Ok(()),
    }
}
