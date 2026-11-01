//! Completed proofs are temporary migration state. Keep partial sources until
//! their family opens, including deduplication keys shared by several files.
use fastctx_storage::{Database, Priority};
use std::io;

pub(super) fn collect(database: &Database) -> io::Result<()> {
    for statement in [
        "DELETE FROM migration_files WHERE (family,path) IN (SELECT f.family,f.path FROM migration_files f WHERE f.archived=1 AND (EXISTS(SELECT 1 FROM conversation_record_imports i WHERE i.family=f.family AND (i.conversation=f.conversation OR i.conversation='@node')) OR (f.family='run-facts' AND EXISTS(SELECT 1 FROM run_imported r WHERE r.conversation=f.conversation AND r.checked=1))) LIMIT 128)",
        "DELETE FROM migration_content_keys WHERE (conversation,family,hash) IN (SELECT k.conversation,k.family,k.hash FROM migration_content_keys k JOIN conversation_record_imports i ON i.conversation=k.conversation AND i.family=k.family LIMIT 128)",
        "DELETE FROM session_sources WHERE (conversation,identity) IN (SELECT s.conversation,s.identity FROM session_sources s JOIN session_heads h ON h.conversation=s.conversation WHERE h.imported=1 LIMIT 128)",
        "DELETE FROM session_migration_state WHERE conversation IN (SELECT s.conversation FROM session_migration_state s JOIN session_heads h ON h.conversation=s.conversation WHERE h.imported=1 LIMIT 128)",
    ] {
        loop {
            let removed = database
                .write_now_at(Priority::Background, 128 * 128, move |db| Ok(db.execute(statement, [])?))
                .map_err(io::Error::other)?;
            if removed == 0 {
                break;
            }
        }
    }
    Ok(())
}
