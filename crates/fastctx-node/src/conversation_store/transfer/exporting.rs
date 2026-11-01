//! Offline export uses the node's exclusive ownership lock, so the database
//! snapshot and the files copied by the operator describe one stopped node.
use super::{bundle, directory};
use fastctx_storage::rusqlite::{Connection, OpenFlags, OptionalExtension};
use fastctx_types::layout::WorldDir;
use std::{
    io,
    path::{Path, PathBuf},
};

pub(crate) fn export(root: &Path, conversation: &str) -> io::Result<PathBuf> {
    let root = std::path::absolute(root)?;
    let world = WorldDir::under(&root);
    let _owner = fastctx_platform::FileLock::try_acquire(&world.node_lock())?.ok_or_else(|| {
        io::Error::new(io::ErrorKind::WouldBlock, "stop the source node before exporting a conversation")
    })?;
    if !world.kept()?.iter().any(|kept| kept.name == conversation) {
        return Err(io::Error::new(io::ErrorKind::NotFound, "the conversation is not kept on this node"));
    }
    let export = || -> fastctx_storage::Result<PathBuf> {
        let source = Connection::open_with_flags(world.path().join("node.sqlite3"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        super::super::check(&source, conversation)?;
        let complete = source
            .query_row("SELECT value FROM node_record_state WHERE name='legacy-discovery-complete'", [], |row| {
                row.get::<_, bool>(0)
            })
            .optional()?
            .unwrap_or(false);
        if !complete {
            return Err(fastctx_storage::Error::new(
                "legacy discovery is not complete; let the source node finish migration before stopping it",
            ));
        }
        for table in ["migration_files", "migration_content_keys", "session_sources", "session_migration_state"] {
            let pending: bool = source.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE conversation=?1)"),
                [conversation],
                |row| row.get(0),
            )?;
            if pending {
                return Err(fastctx_storage::Error::new(format!(
                    "conversation migration proofs remain in {table}; let source maintenance finish first"
                )));
            }
        }
        let pending: bool = source.query_row(
            "SELECT EXISTS(SELECT 1 FROM node_conversation_transfers WHERE name=?1 AND rejected=0)",
            [conversation],
            |row| row.get(0),
        )?;
        if pending {
            return Err(fastctx_storage::Error::new("the conversation is still being imported"));
        }
        let directory = directory(world.path(), conversation)?;
        let machine = crate::machine::names(&world).map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
        bundle::write(&source, directory.path(), conversation, machine.name.as_str())?;
        Ok(directory.path().join(bundle::FILE))
    };
    export().map_err(io::Error::other)
}
