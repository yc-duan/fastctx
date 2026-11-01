//! Completed proofs are temporary migration state. Keep partial sources until
//! their family opens, including deduplication keys shared by several files.
mod files;
use fastctx_storage::{Database, Priority};
use std::io;

/// Archive retention, damaged originals' too, and completed-proof cleanup do
/// not wait for every source on the machine to become readable.
pub(super) fn start(node: &std::sync::Arc<crate::Node>) {
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        loop {
            let working = node.clone();
            let result = fastctx_platform::task::spawn_blocking(move || {
                collect(&working.database)?;
                super::archive::damaged_kept(working.world.path(), super::archive::RETENTION)?;
                super::archive::collect(working.world.path(), super::archive::RETENTION)
            })
            .await;
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => node.log.line(format_args!("maintaining legacy archives: {error}")),
                Err(error) => node.log.line(format_args!("archive maintenance worker: {error}")),
            }
            tokio::time::sleep(std::time::Duration::from_secs(60 * 60)).await;
        }
    });
}

pub(super) fn collect(database: &Database) -> io::Result<()> {
    files::collect(database)?;
    for statement in [
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
