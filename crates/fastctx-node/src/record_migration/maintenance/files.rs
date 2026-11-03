//! Drain each completed file's small row proofs before its checkpoint, so empty
//! files are removed from the archived index instead of scanned every batch.
use fastctx_storage::{Database, Priority};
use std::io;

/// The verified sources of completed families that were not moved yet, as
/// another program held them open then: each family's, a few at a time.
pub(super) fn archive_left(database: &Database, world: &std::path::Path) -> io::Result<()> {
    let families = database
        .read_background_now(|db| {
            Ok(db
                .prepare(
                    "SELECT DISTINCT f.conversation,f.family FROM migration_files f WHERE f.verified=1 AND f.archived=0
                AND (EXISTS(SELECT 1 FROM conversation_record_imports i WHERE i.family=f.family AND (i.conversation=f.conversation OR i.conversation='@node'))
                OR (f.family='run-facts' AND EXISTS(SELECT 1 FROM run_imported r WHERE r.conversation=f.conversation AND r.checked=1))) LIMIT 64",
                )?
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .map_err(io::Error::other)?;
    for (conversation, family) in families {
        let context = crate::record_migration::files::Context {
            database: database.clone(),
            world: world.to_path_buf(),
            conversation,
            family,
        };
        crate::record_migration::files::archive_ready(&context)?;
    }
    Ok(())
}

pub(super) fn collect(database: &Database) -> io::Result<()> {
    loop {
        let files=database.read_background_now(|db| {
            Ok(db.prepare("SELECT f.family,f.path,f.incarnation FROM migration_files f WHERE f.archived=1
                AND NOT EXISTS(SELECT 1 FROM node_conversation_transfers t WHERE t.name=f.conversation AND t.rejected=0)
                AND (EXISTS(SELECT 1 FROM conversation_record_imports i WHERE i.family=f.family AND (i.conversation=f.conversation OR i.conversation='@node'))
                OR (f.family='run-facts' AND EXISTS(SELECT 1 FROM run_imported r WHERE r.conversation=f.conversation AND r.checked=1))) LIMIT 32")?
                .query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?
                .collect::<std::result::Result<Vec<_>,_>>()?)
        }).map_err(io::Error::other)?;
        if files.is_empty() {
            return Ok(());
        }
        // What goes here is done with and found again if lost, so a batch of
        // files goes in relaxed commits rather than waiting on the disk for
        // each file twice.
        loop {
            let batch = files.clone();
            let removed = database
                .write_relaxed_now_at(Priority::Background, 16 * 1024, move |db| {
                    let mut delete = db.prepare_cached("DELETE FROM migration_row_proofs WHERE family=?1 AND path=?2 AND ordinal IN (
                        SELECT ordinal FROM migration_row_proofs WHERE family=?1 AND path=?2 LIMIT 128)
                        AND EXISTS(SELECT 1 FROM migration_files WHERE family=?1 AND path=?2 AND incarnation=?3 AND archived=1)")?;
                    let mut removed = 0;
                    for (family, path, incarnation) in &batch {
                        removed += delete.execute((family, path, incarnation))?;
                    }
                    Ok(removed)
                })
                .map_err(io::Error::other)?;
            if removed == 0 {
                break;
            }
        }
        database
            .write_relaxed_now_at(Priority::Background, 256 * files.len(), move |db| {
                let mut delete = db.prepare_cached(
                    "DELETE FROM migration_files WHERE family=?1 AND path=?2 AND incarnation=?3 AND archived=1",
                )?;
                for (family, path, incarnation) in &files {
                    delete.execute((family, path, incarnation))?;
                }
                Ok(())
            })
            .map_err(io::Error::other)?;
    }
}
