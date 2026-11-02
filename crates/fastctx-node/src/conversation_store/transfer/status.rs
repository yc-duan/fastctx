//! Read-only inspection also works while the node is stopped or recovering.
use fastctx_storage::rusqlite::{Connection, OpenFlags};
use fastctx_types::layout::WorldDir;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

/// Where a conversation moved by hand stands on this machine: a pending
/// import or rejected offer, or the last export it took whole or let go of,
/// by its fingerprint. A rejected offer no longer fences the conversation;
/// its reason and archived bytes remain available to operators.
#[derive(Debug, serde::Serialize)]
pub struct ConversationImport {
    pub conversation: String,
    pub state: String,
    pub reason: Option<String>,
    /// Where its bytes were set aside: a rejected offer's, or the directory
    /// of a conversation let go.
    pub archive: Option<PathBuf>,
    /// The export taken, by the name the other machine lets it go by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// The machine an export taken came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

pub(crate) fn list(root: &Path) -> io::Result<Vec<ConversationImport>> {
    let world = WorldDir::under(&std::path::absolute(root)?);
    let read = || -> fastctx_storage::Result<Vec<ConversationImport>> {
        let db = Connection::open_with_flags(world.path().join("node.sqlite3"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut found = BTreeMap::new();
        // A node last run before moves were kept has none to say.
        let moves: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='node_conversation_releases')",
            [],
            |row| row.get(0),
        )?;
        if moves {
            let mut query =
                db.prepare("SELECT name,fingerprint,kept FROM node_conversation_releases ORDER BY name,at")?;
            for release in query.query_map([], |row| {
                Ok(ConversationImport {
                    conversation: row.get(0)?,
                    state: "released".into(),
                    reason: None,
                    archive: Some(PathBuf::from(row.get::<_, String>(2)?)),
                    fingerprint: Some(row.get(1)?),
                    from: None,
                })
            })? {
                let release = release?;
                found.insert(release.conversation.clone(), release);
            }
            let mut query = db.prepare("SELECT name,fingerprint,source_machine FROM node_conversation_arrivals")?;
            for arrival in query.query_map([], |row| {
                Ok(ConversationImport {
                    conversation: row.get(0)?,
                    state: "imported".into(),
                    reason: None,
                    archive: None,
                    fingerprint: Some(row.get(1)?),
                    from: Some(row.get(2)?),
                })
            })? {
                let arrival = arrival?;
                found.insert(arrival.conversation.clone(), arrival);
            }
        }
        let mut query = db.prepare("SELECT name,rejected,failure,archive FROM node_conversation_transfers")?;
        for pending in query.query_map([], |row| {
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
                fingerprint: None,
                from: None,
            })
        })? {
            let pending = pending?;
            found.insert(pending.conversation.clone(), pending);
        }
        Ok(found.into_values().collect())
    };
    read().map_err(io::Error::other)
}
