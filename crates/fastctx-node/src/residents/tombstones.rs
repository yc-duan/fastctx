//! What is remembered of a conversation deleted from this machine: when it
//! went, and the sessions its host knew it by.
//!
//! Whatever still arrives for it afterwards (a report from one of its Claude
//! sessions, a machine that watches it, words queued for it) is answered as
//! deleted and dropped, never taken for a new conversation, and its id goes
//! to no other one. The tombstone is written before the conversation's
//! records go, so a node stopped halfway finishes the deletion when it
//! starts again. It is kept for a month.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use fastctx_platform::Permissions;
use fastctx_types::ResidentId;
use fastctx_types::layout::WorldDir;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

/// How long a deleted conversation is remembered.
const KEPT: SignedDuration = SignedDuration::from_hours(30 * 24);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tombstone {
    pub at: Timestamp,
    /// The sessions its host knew it by: Claude Code's sessions, or its
    /// Codex thread.
    pub sessions: Vec<String>,
}

#[derive(Default)]
pub(super) struct Tombstones {
    by_id: HashMap<ResidentId, Tombstone>,
    by_session: HashMap<String, ResidentId>,
}

impl Tombstones {
    /// Those kept under `world`, the ones older than a month removed. One
    /// that cannot be read is reported and left as it is.
    pub(super) fn load(world: &WorldDir, problems: &mut Vec<(PathBuf, io::Error)>) -> io::Result<Self> {
        let mut tombstones = Self::default();
        let entries = match std::fs::read_dir(world.deleted()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(tombstones),
            Err(error) => return Err(error),
        };
        let now = Timestamp::now();
        for entry in entries {
            let path = entry?.path();
            let Some(id) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|id| ResidentId::try_from(id).ok())
            else {
                continue;
            };
            let tombstone = match std::fs::read(&path)
                .and_then(|bytes| serde_json::from_slice::<Tombstone>(&bytes).map_err(io::Error::other))
            {
                Ok(tombstone) => tombstone,
                Err(error) => {
                    problems.push((path, error));
                    continue;
                }
            };
            if now.duration_since(tombstone.at) > KEPT {
                match std::fs::remove_file(&path) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => problems.push((path, error)),
                    _ => {}
                }
                continue;
            }
            tombstones.insert(id, tombstone);
        }
        Ok(tombstones)
    }

    /// Remembers that `id` is deleted, on disk first.
    pub(super) fn bury(&mut self, world: &WorldDir, id: &ResidentId, tombstone: Tombstone) -> io::Result<()> {
        let directory = world.deleted();
        fastctx_platform::ensure_private_dir(&directory)?;
        let bytes = serde_json::to_vec_pretty(&tombstone).map_err(io::Error::other)?;
        fastctx_platform::publish(&directory.join(format!("{id}.json")), &bytes, Permissions::Private)?;
        self.insert(id.clone(), tombstone);
        Ok(())
    }

    fn insert(&mut self, id: ResidentId, tombstone: Tombstone) {
        for session in &tombstone.sessions {
            self.by_session.insert(session.clone(), id.clone());
        }
        self.by_id.insert(id, tombstone);
    }

    pub(super) fn of(&self, id: &ResidentId) -> Option<&Tombstone> {
        self.by_id.get(id)
    }

    /// The deleted conversation the host session `session` was, if any.
    pub(super) fn holding(&self, session: &str) -> Option<&ResidentId> {
        self.by_session.get(session)
    }

    pub(super) fn all(&self) -> impl Iterator<Item = (&ResidentId, &Tombstone)> {
        self.by_id.iter()
    }
}
