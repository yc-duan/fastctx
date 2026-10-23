//! Durable account changes and their local effects, recovered by the conversation that owns them.

use fastctx_types::Address;
use fastctx_types::access::HubAccount;
use fastctx_types::goals::Goal;
use fastctx_types::layout::ConversationDir;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Intent {
    pub operation: String,
    pub resident: Address,
    pub input_hash: String,
    pub account: HubAccount,
    pub expected: Option<u64>,
    pub proposed: Option<Goal>,
    pub at: Timestamp,
    pub phase: Phase,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum Phase {
    Prepared,
    Accepted { version: u64 },
    Applied { version: u64 },
    Conflict { current: Option<u64> },
    Superseded { current: Option<u64> },
}

impl Phase {
    pub fn pending(&self) -> bool {
        matches!(self, Self::Prepared | Self::Accepted { .. })
    }
}

fn directory(conversation: &ConversationDir) -> PathBuf {
    conversation.books().join("goal-operations")
}

fn path(conversation: &ConversationDir, operation: &str) -> PathBuf {
    directory(conversation).join(format!("{}.json", fastctx_hands::files::content_hash(operation.as_bytes())))
}

pub(crate) fn read(conversation: &ConversationDir, operation: &str) -> io::Result<Option<Intent>> {
    let intent: Option<Intent> = match std::fs::read(path(conversation, operation)) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if intent.as_ref().is_some_and(|intent| intent.operation != operation) {
        return Err(io::Error::other("goal operation identity differs from its record"));
    }
    Ok(intent)
}

pub(crate) fn keep(conversation: &ConversationDir, intent: &Intent) -> io::Result<()> {
    let path = path(conversation, &intent.operation);
    fastctx_platform::ensure_private_dir(path.parent().expect("goal operation directory"))?;
    fastctx_platform::publish(&path, &serde_json::to_vec(intent)?, fastctx_platform::Permissions::Private)
}

pub(crate) fn pending(conversation: &ConversationDir) -> io::Result<Vec<Intent>> {
    let mut result = Vec::new();
    for path in entries(&directory(conversation))? {
        if path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'))
            || path.extension().is_none_or(|extension| extension != "json")
        {
            continue;
        }
        let intent: Intent = serde_json::from_slice(&std::fs::read(path)?)?;
        if intent.phase.pending() {
            result.push(intent);
        }
    }
    result.sort_by(|a, b| a.at.cmp(&b.at).then(a.operation.cmp(&b.operation)));
    Ok(result)
}

fn entries(path: &Path) -> io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
