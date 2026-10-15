//! Deleting conversations kept on this machine, by the names the listing
//! gives them.
//!
//! A conversation that lives here goes with everything it has here: its
//! records and attached files, the commands it ran, the plugin's directories
//! for its Claude sessions, and the record its agent's own program keeps of
//! it. Codex deletes its thread itself (`thread/delete`); for Claude Code the
//! files of its sessions are deleted, after Claude Code's own background
//! session, when the World started one, is stopped and removed through
//! `claude`. A conversation still working, or open in its agent's window, or
//! with a command still running here, is not touched: it is stopped or
//! closed first. The host's record goes first; when that fails nothing else
//! does, so the conversation stays listed and can be deleted again.
//!
//! A conversation being deleted takes nothing new: no words, no turn, no
//! command (`Residents::deleting`). Before its records go, what its Claude
//! sessions reported and is being read is let finish, and a tombstone is
//! written (`residents::Tombstone`): what they report afterwards is
//! dropped, and a session of it never comes back as a new conversation. A
//! node stopped after that finishes the deletion when it starts again.
//!
//! A guest room goes alone; the conversation's home on its own machine is
//! not touched. One with a command still running here is not touched.

use std::sync::Arc;

use fastctx_types::data::{Deletion, DeletionOutcome};
use fastctx_types::layout::Kind;
use fastctx_types::residents::{About, Activity};

use super::space::Measured;
use crate::Node;

/// Why a conversation was not deleted.
pub(crate) enum Stop {
    /// Nothing of it is kept here.
    Nothing(String),
    /// Not now, and why: it is working, open, or being read.
    Refused(String),
    Failed(String),
}

impl Stop {
    pub(crate) fn why(self) -> String {
        match self {
            Self::Nothing(why) | Self::Refused(why) | Self::Failed(why) => why,
        }
    }
}

pub(super) async fn delete(node: &Arc<Node>, names: Vec<String>) -> Vec<Deletion> {
    let mut deletions = Vec::with_capacity(names.len());
    for name in names {
        let outcome = match delete_one(node, &name).await {
            Ok(bytes) => DeletionOutcome::Deleted { bytes },
            Err(Stop::Nothing(why) | Stop::Refused(why)) => DeletionOutcome::Refused { why },
            Err(Stop::Failed(why)) => DeletionOutcome::Failed { why },
        };
        if let DeletionOutcome::Deleted { bytes } = &outcome {
            node.log.line(format_args!("deleted {name} from this machine, {bytes} bytes"));
        }
        deletions.push(Deletion { name, outcome });
    }
    deletions
}

/// Deletes everything of the conversation kept here under the name `name`,
/// as the listing gives it; says how many bytes went.
pub(crate) async fn delete_one(node: &Arc<Node>, name: &str) -> Result<u64, Stop> {
    let _operation = node.data_operations.write().await;
    let kind = Kind::of(name).ok_or_else(|| Stop::Refused(format!("{name} is not the name of a conversation")))?;
    let measured = measure(node, name).await?;
    if let Some(job) = measured.running.first() {
        return Err(Stop::Refused(format!(
            "{job}, a command it started, still runs here; stop it first (fastctx stop {job})"
        )));
    }
    let bytes = measured.records.bytes + measured.output.bytes + measured.host.bytes;
    let Kind::Home(id) = &kind else {
        remove_places(node, &measured).await?;
        return Ok(bytes);
    };
    let _deleting = node.residents.deleting(id);
    let directory = node.world.conversation(id);
    let _draft = if directory.draft().exists() || directory.draft_lock().exists() {
        Some(
            fastctx_platform::FileLock::try_acquire(&directory.draft_lock())
                .map_err(|error| Stop::Failed(error.to_string()))?
                .ok_or_else(|| Stop::Refused("it is being opened; wait for it to finish".into()))?,
        )
    } else {
        None
    };
    let Some(resident) = node.residents.on_record(id) else {
        let directory = node.world.conversation(id);
        if directory.about().exists() && !directory.path().join("host-deleted").exists() {
            return Err(Stop::Refused(
                "its identity record cannot be read; its host record cannot be identified".into(),
            ));
        }
        // Files attached for a conversation never opened.
        remove_places(node, &measured).await?;
        return Ok(bytes);
    };
    let gate = node.residents.management(id).map_err(|failure| Stop::Failed(failure.message))?;
    let _exclusive = gate.write().await;
    let busy = node.residents.now(id).is_some_and(|now| now.activity == Activity::Busy || now.turn.is_some());
    if busy {
        return Err(Stop::Refused("it is working; stop it first".to_string()));
    }
    if node.residents.host_open(id) && !resident.about.world_started {
        return Err(Stop::Refused(
            "it is open in its agent's window on this machine; close it there first".to_string(),
        ));
    }
    if resident.about.host.agent.as_str() == "claude" && !resident.about.world_started {
        for exchange in super::space::entries(&node.world.conversation(id).path().join("claude"))
            .map_err(|error| Stop::Failed(error.to_string()))?
        {
            if let Some(registration) = fastctx_hosts::claude::exchange::registration(&exchange)
                .map_err(|error| Stop::Failed(error.to_string()))?
                && let Some(process) = registration.process
                && fastctx_platform::is_alive(&process).map_err(|error| Stop::Failed(error.to_string()))?
            {
                return Err(Stop::Refused(
                    "it is open in its agent's window on this machine; close it there first".into(),
                ));
            }
        }
    }
    if node
        .job_readers
        .lock()
        .expect("never poisoned")
        .keys()
        .any(|path| measured.places.iter().any(|place| path.starts_with(place)))
    {
        return Err(Stop::Refused("its command output is being read; close that reader first".into()));
    }
    let completed = node.world.conversation(id).path().join("host-deleted");
    if !completed.exists() {
        host(node, &resident.about, &measured).await?;
        fastctx_platform::publish(&completed, b"", fastctx_platform::Permissions::Private)
            .map_err(|error| Stop::Failed(format!("recording host deletion: {error}")))?;
    }
    node.residents
        .remove(&node.world, id)
        .map_err(|error| Stop::Failed(format!("recording deletion in progress: {error}")))?;
    // Whatever its sessions reported and is being applied is applied to its
    // end first, in the order applying takes these locks: a Claude session's
    // reading, a Codex thread's worker, then speaking.
    let reading: Vec<_> = measured
        .sessions
        .iter()
        .flat_map(|session| [crate::claude::reading(node, session), crate::codex::working(node, session)])
        .collect();
    let mut _read = Vec::with_capacity(reading.len());
    for session in &reading {
        _read.push(session.lock().await);
    }
    let speaking =
        node.residents.speaking(id).ok_or_else(|| Stop::Failed("its records are no longer registered".into()))?;
    let _speaking = speaking.lock().await;
    // What was kept of its Codex thread's reports while they waited.
    if let Some(thread) = &resident.about.session {
        let reports = node.world.path().join("codex-reports");
        let key = crate::speech::fixed_id("thread", &[thread]);
        for extension in ["json", "jsonl"] {
            let path = reports.join(format!("{key}.{extension}"));
            match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(Stop::Failed(format!("removing {}: {error}", path.display())));
                }
                _ => {}
            }
        }
    }
    node.residents
        .forget(&node.world, id, measured.sessions.clone())
        .map_err(|error| Stop::Failed(format!("recording its deletion: {error}")))?;
    crate::desk::removed(node, id);
    crate::composer::deleted(node, id);
    crate::claude::removed(node, id);
    for session in &measured.sessions {
        crate::codex::deleted(node, session);
    }
    remove_places(node, &measured).await?;
    node.tools_changed(Some(id));
    Ok(bytes)
}

/// Finishes the deletions a node stopped halfway: what is left of a
/// conversation whose tombstone is written goes, with the directories of
/// its Claude sessions, which may no longer say whose they are.
pub(crate) async fn finish_interrupted(node: &Arc<Node>) {
    let root = fastctx_hosts::claude::exchange::sessions(node.world.path());
    for (id, tombstone) in node.residents.tombstones() {
        let sessions: Vec<_> = tombstone
            .sessions
            .iter()
            .filter(|session| crate::claude::pairing::validate_session(session).is_ok())
            .map(|session| root.join(session))
            .collect();
        let left = std::fs::symlink_metadata(node.world.conversation(&id).path()).is_ok()
            || sessions.iter().any(|session| std::fs::symlink_metadata(session).is_ok());
        if !left {
            continue;
        }
        match delete_one(node, id.as_str()).await {
            Ok(_) | Err(Stop::Nothing(_)) => {}
            Err(stop) => {
                node.log.line(format_args!("finishing the deletion of {id}: {}", stop.why()));
                continue;
            }
        }
        let removed = tokio::task::spawn_blocking(move || {
            sessions.iter().try_for_each(|session| fastctx_platform::owned_directory::delete(session))
        })
        .await;
        match removed {
            Ok(Ok(())) => node.log.line(format_args!("finished deleting {id}, which a stopped node left halfway")),
            Ok(Err(error)) => node.log.line(format_args!("finishing the deletion of {id}: {error}")),
            Err(error) => node.log.line(format_args!("finishing the deletion of {id}: {error}")),
        }
    }
}

async fn measure(node: &Arc<Node>, name: &str) -> Result<Measured, Stop> {
    let (node, wanted) = (node.clone(), name.to_string());
    tokio::task::spawn_blocking(move || super::space::conversations(&node))
        .await
        .map_err(|error| Stop::Failed(error.to_string()))?
        .map_err(|error| Stop::Failed(format!("finding what it keeps here: {error}")))?
        .remove(&wanted)
        .ok_or_else(|| Stop::Nothing(format!("nothing of {name} is kept on this machine")))
}

/// Deletes the record the conversation's agent program keeps of it.
async fn host(node: &Arc<Node>, about: &About, measured: &Measured) -> Result<(), Stop> {
    match about.host.agent.as_str() {
        "codex" => {
            let Some(session) = &about.session else { return Ok(()) };
            let codex = node.codex.of(about).map_err(|failure| Stop::Failed(failure.message))?;
            let deleted = codex
                .delete(session)
                .await
                .map_err(|error| Stop::Failed(format!("Codex did not delete its record: {error}")))?;
            if !deleted && !measured.host_files.is_empty() {
                return Err(Stop::Failed(
                    "Codex no longer knows this thread, but its record files are still here".into(),
                ));
            }
        }
        "claude" => {
            if about.world_started
                && let Some(session) = &about.session
            {
                crate::claude::management::retire(node, about.home.as_deref(), &about.id, session).await.map_err(
                    |failure| Stop::Failed(format!("Claude Code's background session: {}", failure.message)),
                )?;
            }
            let files = measured.host_files.clone();
            tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                for file in files {
                    fastctx_platform::owned_directory::delete(&file)?;
                }
                Ok(())
            })
            .await
            .map_err(|error| Stop::Failed(error.to_string()))?
            .map_err(|error| Stop::Failed(format!("deleting Claude Code's record of it: {error}")))?;
            claude_temp(node, measured).await;
            // Its sessions stood in its own directory before they stood in
            // their project's, and the World trusted that directory for it.
            if about.world_started {
                let directory = node.world.conversation(&about.id).path().to_path_buf();
                let home = about.home.clone();
                tokio::task::spawn_blocking(move || crate::claude::untrust(&directory, home.as_deref()))
                    .await
                    .map_err(|error| Stop::Failed(error.to_string()))?
                    .map_err(|error| {
                        Stop::Failed(format!("taking back its directory's trust in Claude Code: {error}"))
                    })?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Deletes what Claude Code wrote to temporary files for the conversation's
/// sessions (`fastctx_hosts::claude::temp`), and the directory of their
/// working directory there once nothing else is in it. What cannot be
/// deleted is told and left: Claude Code itself never deletes it.
async fn claude_temp(node: &Arc<Node>, measured: &Measured) {
    let env = match crate::claude::environment() {
        Ok(env) => env,
        Err(failure) => {
            node.log.line(format_args!("finding Claude Code's temporary files: {}", failure.message));
            return;
        }
    };
    let root = fastctx_hosts::claude::exchange::sessions(node.world.path());
    let (sessions, log) = (measured.sessions.clone(), node.log.clone());
    let _ = tokio::task::spawn_blocking(move || {
        for session in sessions {
            let Ok(Some(registration)) = fastctx_hosts::claude::exchange::registration(&root.join(&session)) else {
                continue;
            };
            let Some(directory) = registration
                .cwd
                .as_deref()
                .and_then(|cwd| fastctx_hosts::claude::temp::session_directory(&env, cwd, &session))
            else {
                continue;
            };
            if let Err(error) = fastctx_platform::owned_directory::delete(&directory) {
                log.line(format_args!("deleting Claude Code's temporary files {}: {error}", directory.display()));
                continue;
            }
            // Empty now, unless another session ran in the same directory.
            if let Some(parent) = directory.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    })
    .await;
}

/// Deletes every place that holds something of the conversation here.
async fn remove_places(node: &Arc<Node>, measured: &Measured) -> Result<(), Stop> {
    let (node, places) = (node.clone(), measured.places.clone());
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let _collecting = node.disk.lock().expect("never poisoned");
        let _transaction = crate::resident_storage::lock();
        if places
            .iter()
            .any(|place| node.job_readers.lock().expect("never poisoned").keys().any(|path| path.starts_with(place)))
        {
            return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "its command output is being read"));
        }
        // Remove aliases before their targets, so a partial deletion never
        // leaves a path pointing at a different conversation later.
        for place in &places {
            if let Ok(metadata) = std::fs::symlink_metadata(place)
                && fastctx_platform::owned_directory::is_link(&metadata)
            {
                fastctx_platform::owned_directory::delete(place)?;
            }
        }
        for place in places {
            if place.join("host-deleted").exists() {
                let last = ["now", "town", "removed", "about", "host-deleted"];
                for entry in super::space::entries(&place)? {
                    if !entry.file_name().is_some_and(|name| last.iter().any(|last| name == *last)) {
                        fastctx_platform::owned_directory::delete(&entry)?;
                    }
                }
                for name in last {
                    fastctx_platform::owned_directory::delete(&place.join(name))?;
                }
                std::fs::remove_dir(&place)?;
            } else {
                fastctx_platform::owned_directory::delete(&place)?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| Stop::Failed(error.to_string()))?
    .map_err(|error| Stop::Failed(format!("deleting its files here: {error}")))
}
