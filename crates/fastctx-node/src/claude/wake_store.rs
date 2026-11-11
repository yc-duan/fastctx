//! Node-local launch intents and permanent exclusions for unwanted host copies.

use crate::Node;
use fastctx_storage::{Priority, rusqlite::OptionalExtension};
use fastctx_types::{ProcessIdentity, ResidentId, ipc::Failure};
use serde::{Deserialize, Serialize};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS claude_wakes (
    conversation TEXT PRIMARY KEY, body BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS claude_copies (
    identity TEXT PRIMARY KEY, short TEXT NOT NULL, body BLOB NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS claude_copies_short ON claude_copies(short);
";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Intent {
    /// When it was claimed, in seconds of the epoch; none in one kept before
    /// this was.
    #[serde(default)]
    pub at: i64,
    pub token: String,
    pub revision: i64,
    pub session: String,
    pub owner: ProcessIdentity,
    pub before: String,
    pub accepted: bool,
    pub completed: bool,
    pub settled: bool,
    pub quiet_since: Option<jiff::Timestamp>,
    pub quiet_owner: Option<ProcessIdentity>,
    pub last_seen: Option<jiff::Timestamp>,
    pub last: String,
    pub problem: Option<String>,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("keeping Claude wake intent: {error}"))
}

pub(super) fn get(node: &Node, id: &ResidentId) -> Result<Option<Intent>, Failure> {
    node.database
        .read_now(|db| {
            let body: Option<Vec<u8>> = db
                .query_row("SELECT body FROM claude_wakes WHERE conversation=?1", [id.as_str()], |row| row.get(0))
                .optional()?;
            body.as_deref().map(crate::records::decode).transpose()
        })
        .map_err(failure)
}

/// The unique resident key arbitrates concurrent callers in the writer transaction.
pub(super) fn claim(node: &Node, id: &ResidentId, intent: Intent, expected: Option<String>) -> Result<bool, Failure> {
    let id = id.to_string();
    let body = crate::records::encode(&intent).map_err(failure)?;
    node.database
        .write_now_at(Priority::Interactive, body.len(), move |db| {
            let claimed = match expected {
                None => db.execute("INSERT OR IGNORE INTO claude_wakes(conversation,body) VALUES(?1,?2)", (&id, &body))? != 0,
                Some(expected) => db.execute("UPDATE claude_wakes SET body=?2 WHERE conversation=?1 AND json_extract(body,'$.token')=?3 AND json_extract(body,'$.settled')=1", (&id, &body, expected))? != 0,
            };
            if claimed {
                fastctx_jobs::submit(
                    db,
                    &fastctx_jobs::Job { kind: super::wake_once::KIND, key: id.clone(), unit: id, payload: Vec::new() },
                )?;
            }
            Ok(claimed)
        })
        .map_err(failure)
}

/// Whether a wake is in flight here, or one was begun since `since`, in
/// seconds of the epoch: the session Claude Code starts in place of waking
/// one appears while its wake is, or shortly after.
pub(super) fn lately(node: &Node, since: i64) -> Result<bool, Failure> {
    node.database
        .read_now(move |db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM claude_wakes WHERE json_extract(body,'$.settled')=0 OR json_extract(body,'$.at')>=?1)",
                [since],
                |row| row.get(0),
            )?)
        })
        .map_err(failure)
}

/// Has the launch of `id` looked at again: what reconciles it may have been
/// set aside, and nothing else gives an unsettled launch its way out.
pub(super) fn watch(node: &Node, id: &ResidentId) -> Result<(), Failure> {
    let id = id.to_string();
    node.database
        .write_now_at(Priority::Interactive, id.len(), move |db| {
            fastctx_jobs::submit(
                db,
                &fastctx_jobs::Job { kind: super::wake_once::KIND, key: id.clone(), unit: id, payload: Vec::new() },
            )
        })
        .map_err(failure)
}

pub(super) fn put(node: &Node, id: &ResidentId, mut intent: Intent) -> Result<(), Failure> {
    let id = id.to_string();
    let token = intent.token.clone();
    let revision = intent.revision;
    intent.revision += 1;
    let body = crate::records::encode(&intent).map_err(failure)?;
    node.database
        .write_now_at(Priority::Interactive, body.len(), move |db| {
            db.execute(
                "UPDATE claude_wakes SET body=?2 WHERE conversation=?1 AND json_extract(body,'$.token')=?3 AND json_extract(body,'$.settled')=0 AND json_extract(body,'$.revision')=?4",
                (id, body, token, revision),
            )?;
            Ok(())
        })
        .map_err(failure)
}

pub(super) fn clear(node: &Node, id: &ResidentId, token: &str) -> Result<(), Failure> {
    let (id, token) = (id.to_string(), token.to_string());
    node.database
        .write_now_at(Priority::Interactive, id.len() + token.len(), move |db| {
            db.execute(
                "UPDATE claude_wakes SET body=CAST(json_set(body,'$.settled',json('true')) AS BLOB) WHERE conversation=?1 AND json_extract(body,'$.token')=?2",
                (id, token),
            )?;
            Ok(())
        })
        .map_err(failure)
}

/// A stale observation cannot settle a generation another observer changed.
pub(super) fn settle_seen(node: &Node, id: &ResidentId, intent: &Intent) -> Result<bool, Failure> {
    let (id, token, revision) = (id.to_string(), intent.token.clone(), intent.revision);
    node.database.write_now_at(Priority::Interactive, id.len() + token.len(), move |db| {
        Ok(db.execute("UPDATE claude_wakes SET body=CAST(json_set(body,'$.settled',json('true'),'$.revision',?4) AS BLOB) WHERE conversation=?1 AND json_extract(body,'$.token')=?2 AND json_extract(body,'$.revision')=?3", (id, token, revision, revision + 1))? != 0)
    }).map_err(failure)
}

pub(super) fn completed(node: &Node, id: &ResidentId, token: &str, accepted: bool) -> Result<(), Failure> {
    let (id, token) = (id.to_string(), token.to_string());
    node.database.write_now_at(Priority::Interactive, id.len() + token.len(), move |db| {
        db.execute("UPDATE claude_wakes SET body=CAST(json_set(body,'$.completed',json('true'),'$.accepted',json(?3),'$.revision',json_extract(body,'$.revision')+1) AS BLOB) WHERE conversation=?1 AND json_extract(body,'$.token')=?2 AND json_extract(body,'$.settled')=0", (id, token, if accepted { "true" } else { "false" }))?;
        Ok(())
    }).map_err(failure)
}
