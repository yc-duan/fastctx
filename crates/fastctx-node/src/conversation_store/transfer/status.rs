//! Read-only inspection also works while the node is stopped or recovering.
use fastctx_storage::rusqlite::{Connection, OpenFlags};
use fastctx_types::layout::WorldDir;
use std::{
    io,
    path::{Path, PathBuf},
};

/// A durable pending import or rejected offer. A rejected offer no longer fences
/// the conversation; its reason and archived bytes remain available to operators.
#[derive(Debug, serde::Serialize)]
pub struct ConversationImport {
    pub conversation: String,
    pub state: String,
    pub reason: Option<String>,
    pub archive: Option<PathBuf>,
}

pub(crate) fn list(root: &Path) -> io::Result<Vec<ConversationImport>> {
    let world = WorldDir::under(&std::path::absolute(root)?);
    let read = || -> fastctx_storage::Result<Vec<ConversationImport>> {
        let db = Connection::open_with_flags(world.path().join("node.sqlite3"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut query =
            db.prepare("SELECT name,rejected,failure,archive FROM node_conversation_transfers ORDER BY name")?;
        Ok(query
            .query_map([], |row| {
                let reason: Option<String> = row.get(2)?;
                let rejected: bool = row.get(1)?;
                Ok(ConversationImport {
                    conversation: row.get(0)?,
                    state: if rejected {
                        "rejected"
                    } else if reason.is_some() {
                        "rejecting"
                    } else {
                        "importing"
                    }
                    .into(),
                    reason,
                    archive: row.get::<_, Option<String>>(3)?.map(PathBuf::from),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    };
    read().map_err(io::Error::other)
}
