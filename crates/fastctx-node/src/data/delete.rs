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
//! dropped, and a session of it never comes back as a new conversation.
//!
//! Once its host's record is gone it is deleted, whatever fails after: the
//! answer says so, with what is still to clear, it takes nothing new, and
//! the node finishes the rest by itself, as it does for a node stopped
//! halfway (`finish_interrupted`). A failure before that cancels the
//! deletion: it stays listed and can be deleted again.
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
            Ok(Gone { bytes, finishing }) => {
                match &finishing {
                    None => node.log.line(format_args!("deleted {name} from this machine, {bytes} bytes")),
                    Some(why) => node.log.line(format_args!("deleted {name} from this machine, finishing: {why}")),
                }
                DeletionOutcome::Deleted { bytes, finishing }
            }
            Err(Stop::Nothing(why) | Stop::Refused(why)) => DeletionOutcome::Refused { why },
            Err(Stop::Failed(why)) => DeletionOutcome::Failed { why },
        };
        deletions.push(Deletion { name, outcome });
    }
    deletions
}

/// The file that says a conversation is being deleted, written before
/// anything outside is done for it.
pub(crate) const INTENT: &str = "deleting";

/// What a deletion freed, and why some of it is still being cleared.
pub(crate) struct Gone {
    pub(crate) bytes: u64,
    pub(crate) finishing: Option<String>,
}

/// Deletes everything of the conversation kept here under the name `name`,
/// as the listing gives it; says how many bytes went.
pub(crate) async fn delete_one(node: &Arc<Node>, name: &str) -> Result<Gone, Stop> {
    static SLOTS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> = std::sync::LazyLock::new(|| {
        let slots = std::thread::available_parallelism().map_or(2, |cores| cores.get().clamp(2, 8));
        Arc::new(tokio::sync::Semaphore::new(slots))
    });
    let permit = tokio::time::timeout(std::time::Duration::from_secs(10), SLOTS.clone().acquire_owned())
        .await
        .map_err(|_| Stop::Refused("conversation deletion is busy; retry shortly".into()))?
        .expect("deletion slots remain open");
    let (node, name, runtime) = (node.clone(), name.to_string(), tokio::runtime::Handle::current());
    fastctx_platform::task::spawn_blocking(move || {
        let _permit = permit;
        runtime.block_on(delete_on_worker(&node, &name))
    })
    .await
    .map_err(|error| Stop::Failed(error.to_string()))?
}

async fn delete_on_worker(node: &Arc<Node>, name: &str) -> Result<Gone, Stop> {
    let mut releases = match Kind::of(name) {
        Some(Kind::Home(id)) => crate::forget::release_attachments(node, &id).await.map_err(Stop::Failed)?,
        _ => Vec::new(),
    };
    if let Some(Kind::Home(id)) = Kind::of(name)
        && let Some(parent) = crate::forks::parent(node, &id).map_err(|error| Stop::Failed(error.to_string()))?
    {
        releases.push(parent);
    }
    let mut gone = delete_inner(node, name).await?;
    let mut seen = std::collections::HashSet::new();
    while let Some(owner) = releases.pop() {
        if !seen.insert(owner.clone()) {
            continue;
        }
        if let Some(parent) =
            crate::forks::parent(node, &owner.resident).map_err(|error| Stop::Failed(error.to_string()))?
        {
            releases.push(parent);
        }
        match delete_inner(node, owner.resident.as_str()).await {
            Ok(freed) => {
                gone.bytes += freed.bytes;
                gone.finishing = gone.finishing.or(freed.finishing);
            }
            Err(Stop::Nothing(_)) => {}
            Err(error) => return Err(error),
        }
    }
    crate::forget::wake(node);
    Ok(gone)
}

async fn delete_inner(node: &Arc<Node>, name: &str) -> Result<Gone, Stop> {
    let _doing = fastctx_platform::doing::doing(format!("deleting {name}"));
    let kind = Kind::of(name).ok_or_else(|| Stop::Refused(format!("{name} is not the name of a conversation")))?;
    let actions = match &kind {
        Kind::Home(id) => Some(crate::control_queue::gate(node, id)),
        _ => None,
    };
    let _action = match &actions {
        Some(gate) => {
            let Kind::Home(id) = &kind else { unreachable!() };
            Some(crate::gate_waits::action(node, id, gate).await.map_err(|failure| Stop::Refused(failure.message))?)
        }
        None => None,
    };
    // Its place alone: no command starts for it, no output of it is read
    // and no file arrives for it until this is done; other places go on.
    let _place = node.places.deleting(name).await.ok_or_else(|| {
        Stop::Refused("a command is starting for it, its output is being read or a file is arriving for it; try again in a moment".into())
    })?;
    // From here it takes no new words, turns or commands.
    let _deleting = match &kind {
        Kind::Home(id) => Some(node.residents.deleting(id)),
        _ => None,
    };
    let measured = measure(node, name).await?;
    if let Some(job) = measured.running.first() {
        return Err(Stop::Refused(format!(
            "{job}, a command it started, still runs here; stop it first (fastctx stop {job})"
        )));
    }
    let bytes = measured.records.bytes + measured.output.bytes + measured.host.bytes;
    let Kind::Home(id) = &kind else {
        // Its activity here goes with it, and nothing told of it later is taken.
        crate::activity_book::seal(node, name)
            .map_err(|error| Stop::Failed(format!("sealing its activity: {error}")))?;
        let (retained, database_bytes) = remove_places(node, &measured).await?;
        return Ok(Gone { bytes: bytes.saturating_sub(retained).saturating_add(database_bytes), finishing: None });
    };
    let directory = node.world.conversation(id);
    let _draft = if directory.draft().try_exists().map_err(|error| Stop::Failed(error.to_string()))?
        || directory.draft_lock().try_exists().map_err(|error| Stop::Failed(error.to_string()))?
    {
        Some(
            fastctx_platform::FileLock::try_acquire(&directory.draft_lock())
                .map_err(|error| Stop::Failed(error.to_string()))?
                .ok_or_else(|| Stop::Refused("it is being opened; wait for it to finish".into()))?,
        )
    } else {
        None
    };
    let Some(resident) = node.residents.on_record(id) else {
        crate::goals::lifecycle::prepare_delete(node, id).await.map_err(|error| Stop::Failed(error.message))?;
        crate::goals::lifecycle::delete(node, id).await.map_err(|error| Stop::Failed(error.message))?;
        let directory = node.world.conversation(id);
        if directory.about().try_exists().map_err(|error| Stop::Failed(error.to_string()))?
            && !directory.path().join("host-deleted").try_exists().map_err(|error| Stop::Failed(error.to_string()))?
        {
            return Err(Stop::Refused(
                "its identity record cannot be read; its host record cannot be identified".into(),
            ));
        }
        let mut host_kept = 0;
        if let Some(about) = crate::forks::retained(&directory).map_err(|error| Stop::Failed(error.to_string()))? {
            if crate::forks::has_dependents(node, id).map_err(|error| Stop::Failed(error.to_string()))? {
                host_kept = measured.host.bytes;
            } else {
                host(node, &about, &measured).await?;
                fastctx_platform::owned_directory::delete(&directory.path().join(crate::forks::RETAINED_HOST))
                    .map_err(|error| Stop::Failed(error.to_string()))?;
            }
        }
        crate::activity_book::seal(node, name)
            .map_err(|error| Stop::Failed(format!("sealing its activity: {error}")))?;
        let (retained, database_bytes) = remove_places(node, &measured).await?;
        return Ok(Gone {
            bytes: bytes.saturating_sub(retained + host_kept).saturating_add(database_bytes),
            finishing: None,
        });
    };
    let gate = node.residents.management(id).map_err(|failure| Stop::Failed(failure.message))?;
    let _exclusive =
        crate::gate_waits::alone(node, id, &gate).await.map_err(|failure| Stop::Refused(failure.message))?;
    let busy = node.residents.now(id).is_some_and(|now| now.activity == Activity::Busy || now.turn.is_some());
    if busy {
        return Err(Stop::Refused("it is working; stop it first".to_string()));
    }
    if node.residents.host_open(id) && !resident.about.world_managed() {
        return Err(Stop::Refused(
            "it is open in its agent's window on this machine; close it there first".to_string(),
        ));
    }
    if resident.about.host.agent.as_str() == "claude" && !resident.about.world_managed() {
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
    crate::attachments::lifetime::ensure_holders(node, id).await.map_err(|error| Stop::Failed(error.message))?;
    crate::goals::lifecycle::prepare_delete(node, id).await.map_err(|error| Stop::Failed(error.message))?;
    // The intent: a node stopped from here on finishes the deletion when it
    // starts again (`finish_interrupted`).
    fastctx_platform::publish(
        &node.world.conversation(id).path().join(INTENT),
        b"",
        fastctx_platform::Permissions::Private,
    )
    .map_err(|error| Stop::Failed(format!("recording that it is being deleted: {error}")))?;
    let host_kept = match host_record(node, id, &resident, &measured).await {
        Ok(kept) => kept,
        Err(stop) => {
            // Nothing that cannot be undone was done: it stays as it was,
            // and the node does not finish this deletion later.
            if let Err(error) = fastctx_platform::disk::remove_file(directory.path().join(INTENT)) {
                node.log.line(format_args!("cancelling the deletion of {id}: {error}"));
            }
            return Err(stop);
        }
    };
    // From here it is deleted: what fails now is finished by the node.
    node.residents.condemn(id);
    match records(node, id, name, &resident, &measured).await {
        Ok((retained, database_bytes)) => Ok(Gone {
            bytes: bytes.saturating_sub(retained + host_kept).saturating_add(database_bytes),
            finishing: None,
        }),
        Err(stop) => Ok(Gone { bytes: bytes.saturating_sub(host_kept), finishing: Some(stop.why()) }),
    }
}

/// Deletes the record its host keeps of it, unless a fork still needs it;
/// says how many bytes of it are kept.
async fn host_record(
    node: &Arc<Node>,
    id: &fastctx_types::ResidentId,
    resident: &crate::residents::OnRecord,
    measured: &Measured,
) -> Result<u64, Stop> {
    let directory = node.world.conversation(id);
    let completed = directory.path().join("host-deleted");
    if completed.try_exists().map_err(|error| Stop::Failed(error.to_string()))? {
        return Ok(0);
    }
    if resident.about.host.agent.as_str() == "codex"
        && crate::forks::has_dependents(node, id).map_err(|error| Stop::Failed(error.to_string()))?
    {
        crate::forks::keep(node, &resident.about).map_err(|error| Stop::Failed(error.to_string()))?;
        if resident.about.session.is_some() {
            crate::lifecycle::release_locked(node, &resident.about, crate::lifecycle::ReleaseReason::Archive, None)
                .await
                .map_err(|error| Stop::Failed(error.message))?;
        }
        return Ok(measured.host.bytes);
    }
    host(node, &resident.about, measured).await?;
    fastctx_platform::owned_directory::delete(&directory.path().join(crate::forks::RETAINED_HOST))
        .map_err(|error| Stop::Failed(error.to_string()))?;
    fastctx_platform::publish(&completed, b"", fastctx_platform::Permissions::Private)
        .map_err(|error| Stop::Failed(format!("recording host deletion: {error}")))?;
    Ok(0)
}

/// Everything else of a conversation whose host's record is gone: its
/// listing, plan, goals, records and places; says how many bytes of its
/// places are kept, and how many of its records went.
async fn records(
    node: &Arc<Node>,
    id: &fastctx_types::ResidentId,
    name: &str,
    resident: &crate::residents::OnRecord,
    measured: &Measured,
) -> Result<(u64, u64), Stop> {
    let directory = node.world.conversation(id);
    // Its activity here goes with it, and nothing told of it later is taken.
    crate::activity_book::seal(node, name).map_err(|error| Stop::Failed(format!("sealing its activity: {error}")))?;
    node.residents
        .remove(&node.world, id)
        .map_err(|error| Stop::Failed(format!("recording deletion in progress: {error}")))?;
    crate::plan::delete(node, &resident.about).await.map_err(|failure| Stop::Failed(failure.message))?;
    crate::goals::lifecycle::delete(node, id).await.map_err(|error| Stop::Failed(error.message))?;
    // Whatever its sessions reported and is being applied is applied to its
    // end first, in the order applying takes these locks: a Claude session's
    // reading, a Codex thread's worker, then speaking.
    // In the order of locks: every Claude session's, then the Codex thread's,
    // each by its id (`fastctx_platform::doing::Rank`).
    let mut sessions = measured.sessions.clone();
    sessions.sort();
    sessions.dedup();
    let reading: Vec<_> = sessions
        .iter()
        .map(|session| crate::claude::reading(node, session))
        .chain(sessions.iter().map(|session| crate::codex::working(node, session)))
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
            match fastctx_platform::disk::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(Stop::Failed(format!("removing {}: {error}", path.display())));
                }
                _ => {}
            }
        }
    }
    crate::sentinels::deletion::note_deleted(node, id).map_err(Stop::Failed)?;
    crate::conversation_store::begin(&directory).map_err(|error| Stop::Failed(error.to_string()))?;
    crate::conversation_store::purge(&node.database, id.as_str()).map_err(|error| Stop::Failed(error.to_string()))?;
    node.residents
        .forget(&node.world, id, measured.sessions.clone())
        .map_err(|error| Stop::Failed(format!("recording its deletion: {error}")))?;
    crate::codex::native_context::forget(node, id);
    crate::desk::removed(node, id);
    crate::composer::deleted(node, id).await.map_err(|error| Stop::Failed(error.message))?;
    crate::claude::removed(node, id);
    for session in &measured.sessions {
        crate::codex::deleted(node, session, &node.world.conversation(id))
            .await
            .map_err(|error| Stop::Failed(error.to_string()))?;
    }
    let result = remove_places(node, measured).await?;
    node.tools_changed(Some(id));
    Ok(result)
}

/// Finishes the deletions a node stopped halfway: what is left of a
/// conversation whose tombstone is written goes, with the directories of
/// its Claude sessions, which may no longer say whose they are. Says how
/// many could not be finished now, including scans whose outcome is unknown.
pub(crate) async fn finish_interrupted(node: &Arc<Node>) -> usize {
    let (node, runtime) = (node.clone(), tokio::runtime::Handle::current());
    fastctx_platform::task::spawn_blocking(move || runtime.block_on(finish_on_worker(&node))).await.unwrap_or(1)
}

async fn finish_on_worker(node: &Arc<Node>) -> usize {
    let recovering = node.clone();
    let result =
        fastctx_platform::task::spawn_blocking(move || crate::conversation_store::resume(&recovering.database)).await;
    let mut unfinished = usize::from(!matches!(result, Ok(Ok(()))));
    // Deletions that began outside and stopped before their tombstone.
    let mut begun = node
        .database
        .read_now(|db| {
            Ok(db
                .prepare("SELECT name FROM node_conversation_deletions WHERE finished=0 ORDER BY name")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .unwrap_or_else(|error| {
            unfinished += 1;
            node.log.line(format_args!("reading pending deletions: {error}"));
            Vec::new()
        });
    match fastctx_platform::disk::read_dir(node.world.conversations()) {
        Ok(entries) => {
            for entry in entries {
                let intent = entry.and_then(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !matches!(Kind::of(&name), Some(Kind::Home(_))) {
                        return Ok(None);
                    }
                    let path = entry.path();
                    if path.join(INTENT).try_exists()? || path.join("host-deleted").try_exists()? {
                        Ok(Some(name))
                    } else {
                        Ok(None)
                    }
                });
                match intent {
                    Ok(Some(name)) => begun.push(name),
                    Ok(None) => {}
                    Err(error) => {
                        unfinished += 1;
                        node.log.line(format_args!("checking a conversation's deletion intent: {error}"));
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            unfinished += 1;
            node.log.line(format_args!("finding the deletions a stopped node began: {error}"));
        }
    }
    begun.sort();
    begun.dedup();
    for name in begun {
        match delete_one(node, &name).await {
            Ok(_) => node.log.line(format_args!("finished deleting {name}, which a stopped node began")),
            Err(Stop::Nothing(_)) => {
                if let Err(error) = crate::record_migration::archive::delete_conversation(node.world.path(), &name)
                    .and_then(|()| crate::conversation_store::finished(&node.database, name.clone()))
                {
                    unfinished += 1;
                    node.log.line(format_args!("finishing deletion of {name}: {error}"));
                }
            }
            Err(stop) => {
                unfinished += 1;
                node.log.line(format_args!("finishing the deletion of {name}: {}", stop.why()));
            }
        }
    }
    let root = fastctx_hosts::claude::exchange::sessions(node.world.path());
    for (id, tombstone) in node.residents.tombstones() {
        let sessions: Vec<_> = tombstone
            .sessions
            .iter()
            .filter(|session| crate::claude::pairing::validate_session(session).is_ok())
            .map(|session| root.join(session))
            .collect();
        let left = std::iter::once(node.world.conversation(&id).path().to_path_buf())
            .chain(sessions.iter().cloned())
            .try_fold(false, |left, path| match fastctx_platform::disk::symlink_metadata(&path) {
                Ok(_) => Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(left),
                Err(error) => Err(std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))),
            });
        match left {
            Ok(false) => continue,
            Ok(true) => {}
            Err(error) => {
                unfinished += 1;
                node.log.line(format_args!("checking what remains of {id}: {error}"));
                continue;
            }
        }
        match delete_one(node, id.as_str()).await {
            Ok(_) | Err(Stop::Nothing(_)) => {}
            Err(stop) => {
                unfinished += 1;
                node.log.line(format_args!("finishing the deletion of {id}: {}", stop.why()));
                continue;
            }
        }
        let removed = fastctx_platform::task::spawn_blocking(move || {
            sessions.iter().try_for_each(|session| fastctx_platform::owned_directory::delete(session))
        })
        .await;
        match removed {
            Ok(Ok(())) => node.log.line(format_args!("finished deleting {id}, which a stopped node left halfway")),
            Ok(Err(error)) => {
                unfinished += 1;
                node.log.line(format_args!("finishing the deletion of {id}: {error}"));
            }
            Err(error) => {
                unfinished += 1;
                node.log.line(format_args!("finishing the deletion of {id}: {error}"));
            }
        }
    }
    unfinished
}

async fn measure(node: &Arc<Node>, name: &str) -> Result<Measured, Stop> {
    let (node, wanted) = (node.clone(), name.to_string());
    fastctx_platform::task::spawn_blocking(move || {
        if let Some(measured) = super::space::conversation(&node, &wanted)? {
            if let Some(reason) = measured.unreadable_jobs.first() {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, reason.clone()));
            }
            return Ok(Some(measured));
        }
        // Listing uses directory predicates; an inaccessible directory must
        // not become evidence that a deletion has already finished.
        match fastctx_platform::disk::metadata(node.world.conversations().join(&wanted)) {
            Ok(metadata) if metadata.is_dir() => Err(std::io::Error::other(format!("{wanted} was not measured"))),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    })
    .await
    .map_err(|error| Stop::Failed(error.to_string()))?
    .map_err(|error| Stop::Failed(format!("finding what it keeps here: {error}")))?
    .ok_or_else(|| Stop::Nothing(format!("nothing of {name} is kept on this machine")))
}

/// Deletes the record the conversation's agent program keeps of it.
async fn host(node: &Arc<Node>, about: &About, measured: &Measured) -> Result<(), Stop> {
    match about.host.agent.as_str() {
        "codex" => {
            if about.session.is_none() {
                return Ok(());
            }
            let deleted = crate::lifecycle::release_locked(node, about, crate::lifecycle::ReleaseReason::Delete, None)
                .await
                .map_err(|error| Stop::Failed(format!("Codex did not delete its record: {error}")))?;
            // Codex no longer knows the thread, as when an earlier try
            // deleted it and stopped: what is left of its record goes too.
            if !deleted && !measured.host_files.is_empty() {
                let files = measured.host_files.clone();
                fastctx_platform::task::spawn_blocking(move || {
                    files.iter().try_for_each(|file| fastctx_platform::owned_directory::delete(file))
                })
                .await
                .map_err(|error| Stop::Failed(error.to_string()))?
                .map_err(|error| Stop::Failed(format!("deleting what is left of Codex's record of it: {error}")))?;
            }
        }
        "claude" => {
            if about.world_managed() && about.session.is_some() {
                crate::lifecycle::release_locked(node, about, crate::lifecycle::ReleaseReason::Delete, None)
                    .await
                    .map_err(|failure| {
                        Stop::Failed(format!("Claude Code's background session: {}", failure.message))
                    })?;
            }
            let files = measured.host_files.clone();
            fastctx_platform::task::spawn_blocking(move || -> std::io::Result<()> {
                for file in files {
                    fastctx_platform::owned_directory::delete(&file)?;
                }
                Ok(())
            })
            .await
            .map_err(|error| Stop::Failed(error.to_string()))?
            .map_err(|error| Stop::Failed(format!("deleting Claude Code's record of it: {error}")))?;
            claude_temp(node, measured).await?;
            // Its sessions stood in its own directory before they stood in
            // their project's, and the World trusted that directory for it.
            if about.world_managed() {
                let directory = node.world.conversation(&about.id).path().to_path_buf();
                let home = about.home.clone();
                fastctx_platform::task::spawn_blocking(move || crate::claude::untrust(&directory, home.as_deref()))
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
/// working directory there once nothing else is in it. Failure leaves the
/// deletion intent and session registrations available for the next try.
async fn claude_temp(node: &Arc<Node>, measured: &Measured) -> Result<(), Stop> {
    let env = crate::claude::environment().map_err(|failure| Stop::Failed(failure.message))?;
    let root = fastctx_hosts::claude::exchange::sessions(node.world.path());
    let sessions = measured.sessions.clone();
    fastctx_platform::task::spawn_blocking(move || -> std::io::Result<()> {
        for session in sessions {
            let Some(registration) = fastctx_hosts::claude::exchange::registration(&root.join(&session))? else {
                continue;
            };
            let Some(directory) = registration
                .cwd
                .as_deref()
                .and_then(|cwd| fastctx_hosts::claude::temp::session_directory(&env, cwd, &session))
            else {
                continue;
            };
            fastctx_platform::owned_directory::delete(&directory)?;
            // Empty now, unless another session ran in the same directory.
            if let Some(parent) = directory.parent() {
                match fastctx_platform::disk::remove_dir(parent) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                        ) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| Stop::Failed(error.to_string()))?
    .map_err(|error| Stop::Failed(format!("deleting Claude Code's temporary files: {error}")))
}

/// Deletes every place that holds something of the conversation here.
async fn remove_places(node: &Arc<Node>, measured: &Measured) -> Result<(u64, u64), Stop> {
    let remote = crate::attachments::lifetime::remote_retained(node, &measured.places)
        .await
        .map_err(|error| Stop::Failed(format!("checking attachment references: {}", error.message)))?;
    let (node, places) = (node.clone(), measured.places.clone());
    fastctx_platform::task::spawn_blocking(move || -> std::io::Result<(u64, u64)> {
        let mut database_bytes = 0;
        let mut deleted_scopes = Vec::new();
        for place in &places {
            if place.parent() == Some(node.world.conversations().as_path()) {
                let directory = fastctx_types::layout::ConversationDir::at(place);
                crate::conversation_store::begin(&directory)?;
                let (_, conversation) = crate::record_database::get(&directory)?;
                database_bytes +=
                    crate::conversation_store::purge(&node.database, &conversation).map_err(std::io::Error::other)?;
                deleted_scopes.push(conversation);
            }
        }
        let _collecting = node.disk.lock();
        if places
            .iter()
            .any(|place| node.job_readers.lock().expect("never poisoned").keys().any(|path| path.starts_with(place)))
        {
            return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "its command output is being read"));
        }
        let mut aliases = Vec::new();
        let mut directories = Vec::new();
        for place in places {
            match fastctx_platform::disk::symlink_metadata(&place) {
                Ok(metadata) if fastctx_platform::owned_directory::is_link(&metadata) => aliases.push(place),
                Ok(_) => {
                    let retained = crate::attachments::lifetime::remove(&node, &place, &remote)
                        .map_err(std::io::Error::other)?
                        || place.join(crate::forks::RETAINED_HOST).try_exists()?;
                    directories.push((place, retained));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        // Inherited messages can name paths from before a layout migration.
        // Keep their aliases and the alias manifest until their targets can go.
        let retained = directories.iter().any(|(_, retained)| *retained);
        if !retained {
            for alias in &aliases {
                fastctx_platform::owned_directory::delete(alias)?;
            }
        }
        let mut retained_bytes = 0;
        for (place, retained) in directories {
            let last = ["now", "town", "removed", "about", "host-deleted", INTENT];
            for entry in super::space::entries(&place)? {
                let name = entry.file_name();
                let last = name.is_some_and(|name| last.iter().any(|last| name == *last));
                let keep = retained
                    && name.is_some_and(|name| {
                        name == "attachments"
                            || name == "aliases.json"
                            || name == crate::attachments::lifetime::RETAINED
                            || name == crate::forks::RETAINED_HOST
                    });
                if !last && !keep {
                    fastctx_platform::owned_directory::delete(&entry)?;
                }
            }
            for name in last {
                fastctx_platform::owned_directory::delete(&place.join(name))?;
            }
            if retained {
                retained_bytes += super::space::tally(&place)?.bytes;
            } else {
                fastctx_platform::disk::remove_dir(&place)?;
            }
        }
        if retained {
            for alias in aliases {
                retained_bytes += super::space::tally(&alias)?.bytes;
            }
        }
        drop(_collecting);
        for scope in deleted_scopes {
            crate::record_migration::archive::delete_conversation(node.world.path(), &scope)?;
            crate::conversation_store::finished(&node.database, scope)?;
        }
        Ok((retained_bytes, database_bytes))
    })
    .await
    .map_err(|error| Stop::Failed(error.to_string()))?
    .map_err(|error| Stop::Failed(format!("deleting its files here: {error}")))
}
