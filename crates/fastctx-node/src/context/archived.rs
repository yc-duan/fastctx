//! A context body withdrawn from disk also withdraws its migration originals.
//! The SQL intent survives until the file projection and archive cleanup finish.
use crate::records::Rows;
use fastctx_storage::{Priority, Result};
use fastctx_types::{
    ResidentId,
    layout::{ConversationDir, WorldDir},
};
use std::io;

pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS context_archive_withdrawals (
    conversation TEXT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(conversation,name)
) WITHOUT ROWID;";

pub(super) fn mark(rows: &Rows<'_>, name: &str) -> Result<()> {
    rows.connection().execute(
        "INSERT OR IGNORE INTO context_archive_withdrawals(conversation,name) VALUES(?1,?2)",
        (rows.conversation(), name),
    )?;
    Ok(())
}

/// Called under the same file gate as publication of the current SQL projection.
pub(super) fn clear(world: &WorldDir, id: &ResidentId, directory: &ConversationDir) -> io::Result<()> {
    let names = crate::records::read(directory, |rows| {
        Ok(rows
            .connection()
            .prepare_cached("SELECT name FROM context_archive_withdrawals WHERE conversation=?1 ORDER BY name")?
            .query_map([rows.conversation()], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    })?;
    for name in &names {
        if !super::storage::valid(name) {
            return Err(io::Error::other("invalid withdrawn context name"));
        }
        let path = super::storage::directory(world, id).join(format!("{name}.json"));
        crate::record_migration::withdrawal::source(world.path(), id.as_str(), &path)?;
    }
    if !names.is_empty() {
        crate::records::write_at(directory, Priority::Background, names.len() * 64, move |rows| {
            for name in names {
                rows.connection().execute(
                    "DELETE FROM context_archive_withdrawals WHERE conversation=?1 AND name=?2",
                    (rows.conversation(), name),
                )?;
            }
            Ok(())
        })?;
    }
    Ok(())
}
