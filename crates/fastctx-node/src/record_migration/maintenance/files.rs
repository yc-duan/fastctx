//! Drain each completed file's small row proofs before its checkpoint, so empty
//! files are removed from the archived index instead of scanned every batch.
use fastctx_storage::{Database, Priority};
use std::io;

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
        for (family, path, incarnation) in files {
            loop {
                let (family, path, incarnation) = (family.clone(), path.clone(), incarnation.clone());
                let removed=database.write_now_at(Priority::Background,16*1024,move |db| {
                    Ok(db.execute("DELETE FROM migration_row_proofs WHERE family=?1 AND path=?2 AND ordinal IN (
                        SELECT ordinal FROM migration_row_proofs WHERE family=?1 AND path=?2 LIMIT 128)
                        AND EXISTS(SELECT 1 FROM migration_files WHERE family=?1 AND path=?2 AND incarnation=?3 AND archived=1)",(&family,&path,incarnation))?)
                }).map_err(io::Error::other)?;
                if removed == 0 {
                    break;
                }
            }
            database
                .write_now_at(Priority::Background, 256, move |db| {
                    db.execute(
                        "DELETE FROM migration_files WHERE family=?1 AND path=?2 AND incarnation=?3 AND archived=1",
                        (family, path, incarnation),
                    )?;
                    Ok(())
                })
                .map_err(io::Error::other)?;
        }
    }
}
