//! A Claude resident's history as Claude Code keeps it (`world-colleagues.md`,
//! 用户在任何地方和居民说话): each of its sessions read from Claude Code's
//! own record of it, the one host file the World reads (`world.md`), and
//! placed in the turns the World saw. The World's own record says who spoke
//! and how each turn ended; it keeps no copy of the conversation.

use std::collections::HashSet;

mod follow;
pub(crate) mod kept;
pub(crate) mod placement;
pub(crate) use follow::{Accepting, Keep, Onward, PrefixIds, Stamp, key as left_key, onward, window};
#[cfg(any(test, feature = "bench"))]
pub(crate) use follow::read_on;
#[cfg(test)]
pub(crate) use follow::turns;
pub(crate) use placement::place;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fastctx_hosts::claude::{self, exchange, transcript};
use fastctx_types::session::{Happened, Health, HealthKind, SessionEvent};
use fastctx_types::{ProcessIdentity, ResidentId};
use jiff::Timestamp;

use crate::Node;

/// How long a reading waits for Claude Code to write what its mod saw it
/// keep: past it, the record is read as its file holds it then, a reading
/// not kept for the viewers who come next. Well within the 30 s a viewer on
/// this machine waits for its whole opening (`watch_ipc::TIMEOUT`).
const WRITTEN_WITHIN: Duration = Duration::from_secs(10);

/// How many times a reading a session at work overtook is made again before
/// it is given up, each a little later than the one before.
const OVERTAKEN_MOST: u32 = 10;

pub(crate) struct Read {
    pub events: Vec<SessionEvent>,
    pub unread: bool,
    pub unsettled: bool,
    pub unavailable: Vec<String>,
    /// A session's record stopped before a line larger than World reads
    /// (§28.49): what was read of it is no measure of what is kept of it.
    pub refused: bool,
    /// What each event read from such a session is known by (`freeze`), a
    /// set for each session: none the timeline has is written again.
    pub frozen: Vec<std::collections::HashSet<String>>,
}

/// What Claude Code kept of the resident's sessions, oldest first, placed
/// in the World's turns of `recorded` where the World saw them happen; and
/// whether some session or its record could not be read, so that this
/// reading is not the history as its files hold it. A record that holds a
/// line it cannot make out reads the same each time, and is no such failure.
pub(crate) async fn hosted(node: &Arc<Node>, resident: &ResidentId, recorded: &[SessionEvent]) -> Read {
    let root = exchange::sessions(node.world.path());
    let (sessions, problems) = {
        let (reading, whose) = (node.clone(), resident.clone());
        fastctx_platform::task::blocking(move || sessions(&reading, &whose)).await
    };
    // A session that could not be told for a failed read may be one of its
    // own, missing from this reading.
    let mut unread = !problems.is_empty();
    let mut unsettled = false;
    let mut refused = false;
    let mut frozen = Vec::new();
    let mut unavailable = problems;
    let mut events: Vec<_> = unavailable.iter().map(|problem| note(Timestamp::now(), problem.clone())).collect();
    // A session the conversation continued in shows what it copied from an
    // earlier one only there (`continuing`): by the ids of their records.
    let mut earlier = HashSet::new();
    for Session { id, began, process, kept } in sessions {
        let _doing = fastctx_platform::doing::doing(format!("reading Claude session {id}'s record"));
        let prior = fastctx_platform::roles::in_place("copying Claude record identities", || earlier.clone());
        let read = read_whole(node, &id, root.join(&id), kept, process, prior).await;
        fastctx_platform::roles::in_place("preparing Claude history", || {
            if let Ok((read, _)) = &read {
                earlier.extend(read.uuids.iter().cloned());
            }
            match read {
                Ok((read, whole)) => {
                    refused |= read.refused.is_some();
                    // Read before it held all its mod saw kept: read again next
                    // time, not kept.
                    unread |= !whole;
                    unsettled |= !whole;
                    let stopped = read.refused.is_some();
                    let (shown, problem) = session_events(node, resident, &id, began, read.title, read.events, (&read.problems, read.problems_omitted));
                    if stopped {
                        let mut kept = HashSet::new();
                        freeze(&mut kept, &shown);
                        frozen.push(kept);
                    }
                    events.extend(shown);
                    unavailable.extend(problem);
                }
                Err(why) => {
                    unread = true;
                    let text = format!("Claude Code's record of session {id}: {why}");
                    events.push(note(began, text.clone()));
                    unavailable.push(text);
                }
            }
        });
    }
    Read {
        events: fastctx_platform::roles::in_place("placing Claude history", || place(events, recorded)),
        unread,
        unsettled,
        unavailable,
        refused,
        frozen,
    }
}

/// The events of `sessions` (the resident's, before its last), read whole as
/// `hosted` reads them, and the ids of their records; none where one could
/// not be read whole, so that all is read as ever.
async fn earlier(node: &Arc<Node>, resident: &ResidentId, sessions: Vec<Session>) -> Option<(Vec<SessionEvent>, HashSet<String>)> {
    let root = exchange::sessions(node.world.path());
    let (mut events, mut ids) = (Vec::new(), HashSet::new());
    for Session { id, began, process, kept } in sessions {
        let prior = fastctx_platform::roles::in_place("copying Claude record identities", || ids.clone());
        let (read, whole) = read_whole(node, &id, root.join(&id), kept, process, prior).await.ok()?;
        if !whole {
            return None;
        }
        ids.extend(read.uuids.iter().cloned());
        let (shown, _) = session_events(node, resident, &id, began, read.title, read.events, (&read.problems, read.problems_omitted));
        events.extend(shown);
    }
    Some((events, ids))
}

/// What one session's record shows in the history: its title, which belongs
/// to the session and so takes the session's time, read again the same;
/// its events, the user's images found; and a note of what could not be
/// read (`problems`, and how many more past those kept), which is also
/// given apart.
fn session_events(
    node: &Node,
    resident: &ResidentId,
    id: &str,
    began: Timestamp,
    title: Option<String>,
    read: Vec<SessionEvent>,
    problems: (&[String], usize),
) -> (Vec<SessionEvent>, Option<String>) {
    let (problems, omitted) = problems;
    let mut events = Vec::with_capacity(read.len() + 2);
    if let Some(title) = title {
        events.push(SessionEvent { at: began, happened: Happened::Titled { title } });
    }
    events.extend(read.into_iter().map(|mut event| {
        crate::user_images::claude(node, resident, &mut event);
        event
    }));
    let problem = problem_note(id, problems, omitted);
    if let Some(text) = &problem {
        events.push(note(began, text.clone()));
    }
    (events, problem)
}

/// The note of what session `id`'s record holds that could not be read
/// (`problems`, and how many more past those kept), where it holds any.
pub(crate) fn problem_note(id: &str, problems: &[String], omitted: usize) -> Option<String> {
    (!problems.is_empty()).then(|| {
        format!(
            "{} part(s) of Claude Code's record of session {id} could not be read: {}",
            problems.len() + omitted,
            transcript::listed(problems, omitted)
        )
    })
}

/// What `hosted` reads, kept on disk as it is read (design §28.51 ⑤-4):
/// the history placed in the store (`kept::Placed`); whether some session
/// or its record could not be read, or was read before it held all its mod
/// saw kept (`unread`); whether a record stopped short (`refused`) — as
/// `hosted` tells them.
pub(crate) struct HostedKept {
    pub(crate) placed: kept::Placed,
    pub(crate) unread: bool,
    pub(crate) refused: bool,
}

/// `hosted`, each session's record read into `spill` rather than held
/// (`kept::Reading`), and placed there in the World's turns of `recorded`,
/// the context of the conversation in `directory` restored; an event too
/// large for a record offered to `early`. The same sessions, in the same
/// order, waited for and read again as `hosted` does.
pub(crate) async fn hosted_kept(
    node: &Arc<Node>,
    resident: &ResidentId,
    world: Arc<std::sync::Mutex<crate::timeline::world::World>>,
    directory: fastctx_types::layout::ConversationDir,
    spill: crate::timeline::spill::Spill,
    early: Option<crate::timeline::Early>,
) -> Result<(HostedKept, Option<crate::timeline::Early>), String> {
    let root = exchange::sessions(node.world.path());
    let (sessions, problems) = {
        let (reading, whose) = (node.clone(), resident.clone());
        fastctx_platform::task::blocking(move || sessions(&reading, &whose)).await
    };
    let mut unread = !problems.is_empty();
    let mut refused = false;
    let reading = fastctx_platform::task::blocking(move || kept::Reading::new(spill)).await?;
    let reading = Arc::new(std::sync::Mutex::new(reading));
    {
        let mut held = reading.lock().expect("never poisoned");
        for problem in problems {
            held.note(Timestamp::now(), problem);
        }
    }
    for Session { id, began, process, kept } in sessions {
        let _doing = fastctx_platform::doing::doing(format!("reading Claude session {id}'s record"));
        let read = read_whole_kept(node, resident, &id, root.join(&id), kept, process, &reading).await;
        let holding = reading.clone();
        match read {
            Ok((read, whole)) => {
                refused |= read.refused.is_some();
                // Read before it held all its mod saw kept: read again next
                // time, not kept.
                unread |= !whole;
                fastctx_platform::task::blocking(move || holding.lock().expect("never poisoned").accept(&id, began, &read)).await?;
            }
            Err(why) => {
                unread = true;
                let text = format!("Claude Code's record of session {id}: {why}");
                fastctx_platform::task::blocking(move || holding.lock().expect("never poisoned").failed(began, text)).await?;
            }
        }
    }
    let reading = Arc::try_unwrap(reading).map_err(|_| "the reading is held still".to_string())?.into_inner().expect("never poisoned");
    let (placed, early) = fastctx_platform::task::blocking(move || {
        // Placed in the turns of all the World's record (kept on disk,
        // design §28.51 ⑥-1), merged with all of it, its names and keys
        // kept beside it.
        let mut world = world.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let named = world.view(None).and_then(|()| world.name());
        let failed = std::cell::RefCell::new(None);
        let heard = |turn: &str, text: &str| {
            world.heard_in(turn, text).unwrap_or_else(|error| {
                failed.borrow_mut().get_or_insert(error);
                false
            })
        };
        let placed = named.and_then(|(names, sent)| {
            fastctx_platform::roles::in_place("placing Claude history", || {
                let projection = crate::context::input::Projection::open(&directory).map_err(|error| error.to_string())?;
                let mut restore = |event: &mut SessionEvent| projection.restore(event).map_err(|error| error.to_string());
                let world = kept::Placing::Kept { turns: world.turns(&heard), sent, names };
                reading.place(world, &mut restore, &kept::Fresh(directory.clone()), early.as_ref(), &|_| false)
            })
        });
        let placed = match failed.into_inner() {
            Some(error) => Err(error),
            None => placed,
        };
        (placed, early)
    })
    .await;
    Ok((HostedKept { placed: placed?, unread, refused }, early))
}

/// As `read_whole`, the record read into `reading` (`kept::Reading::read`).
async fn read_whole_kept(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    directory: PathBuf,
    kept: Option<String>,
    process: Option<ProcessIdentity>,
    reading: &Arc<std::sync::Mutex<kept::Reading>>,
) -> Result<(transcript::Transcript, bool), String> {
    let (read, whole) = read_kept_into(node, resident, session, directory, kept, process, reading).await?;
    // Stopped before a line larger than World reads (§28.49), as `read_whole`.
    let whole = whole && read.refused.is_none();
    Ok((read, whole))
}

/// Whether `read`, the record read last into `reading`, holds any of `uuids`.
async fn holds_any(
    reading: &Arc<std::sync::Mutex<kept::Reading>>,
    read: transcript::Transcript,
    uuids: Vec<String>,
) -> Result<(transcript::Transcript, bool), String> {
    let holding = reading.clone();
    fastctx_platform::task::blocking(move || {
        let held = holding.lock().expect("never poisoned");
        let mut found = false;
        for uuid in &uuids {
            if held.holds(&read, uuid)? {
                found = true;
                break;
            }
        }
        Ok((read, found))
    })
    .await
}

/// As `read_kept`, each reading of the record an attempt of `reading`.
async fn read_kept_into(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    directory: PathBuf,
    kept: Option<String>,
    process: Option<ProcessIdentity>,
    reading: &Arc<std::sync::Mutex<kept::Reading>>,
) -> Result<(transcript::Transcript, bool), String> {
    let (cuts, path) = {
        let directory = directory.clone();
        fastctx_platform::task::blocking(move || {
            let cuts = super::rewind::cuts(&directory).map_err(|error| error.to_string())?;
            let path = exchange::transcript(&directory)
                .map_err(|error| format!("where it is kept could not be read: {error}"))?
                .ok_or("the World's plugin never said where Claude Code keeps it")?;
            Ok::<_, String>((cuts, path))
        })
        .await?
    };
    // Shared by the readings, not copied for each.
    let cuts = Arc::new(cuts);
    let Some(kept) = kept else { return read_now_kept(node, resident, path, cuts, reading).await.map(|read| (read, true)) };
    let _waiting = fastctx_platform::doing::waiting_outside(
        format!("Claude Code to write {kept} to session {session}'s record"),
        WRITTEN_WITHIN,
    );
    let gave_up = tokio::time::sleep(WRITTEN_WITHIN);
    tokio::pin!(gave_up);
    // Watched before it is read, as `read_kept` watches it.
    let (changed, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        if crate::file_events::may_change(&event) {
            let _ = changed.send(());
        }
    });
    let folder = path.parent().ok_or_else(|| format!("{} has no folder", path.display()))?;
    let watched = watcher.watch(folder, notify::RecursiveMode::NonRecursive).is_ok();
    let mut ended: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>> = match process {
        Some(process) => Box::pin(async move { fastctx_platform::host_process::exited(&process).await }),
        None => Box::pin(std::future::pending()),
    };
    loop {
        let read = read_now_kept(node, resident, path.clone(), cuts.clone(), reading).await?;
        let latest = node.claude.state.lock().expect("never poisoned").stored.get::<String>(session);
        let (read, found) = holds_any(reading, read, std::iter::once(kept.clone()).chain(latest).collect()).await?;
        if found {
            return Ok((read, true));
        }
        tokio::select! {
            Some(()) = changes.recv() => {}
            () = tokio::time::sleep(Duration::from_secs(2)), if !watched => {}
            () = &mut gave_up => {
                {
                    let state = node.claude.state.lock().expect("never poisoned");
                    if state.stored.get::<String>(session).as_deref() == Some(kept.as_str()) {
                        state.stored.remove(&session);
                    }
                }
                node.log.line(format_args!(
                    "Claude Code did not write {kept} to session {session}'s record within {} s; it is read as its file holds it",
                    WRITTEN_WITHIN.as_secs()
                ));
                return Ok((read, false));
            }
            result = &mut ended => {
                result.map_err(|error| format!("observing Claude's process: {error}"))?;
                let read = read_now_kept(node, resident, path.clone(), cuts.clone(), reading).await?;
                let (mut read, found) = holds_any(reading, read, vec![kept.clone()]).await?;
                if !found {
                    read.problems.push("its process ended before its file held the last of what it kept".to_string());
                }
                return Ok((read, true));
            }
        }
    }
}

/// As `read_now`, each reading an attempt of `reading`, the user's images
/// found in each event as it is made (`user_images::claude`).
async fn read_now_kept(
    node: &Arc<Node>,
    resident: &ResidentId,
    path: PathBuf,
    cuts: Arc<Vec<transcript::HistoryCut>>,
    reading: &Arc<std::sync::Mutex<kept::Reading>>,
) -> Result<transcript::Transcript, String> {
    let mut overtaken = 0;
    loop {
        let (reading_path, cuts, holding, node, whose) = (path.clone(), cuts.clone(), reading.clone(), node.clone(), resident.clone());
        let read = fastctx_platform::task::spawn_blocking(move || {
            holding.lock().expect("never poisoned").read(&reading_path, &cuts, &mut |event| crate::user_images::claude(&node, &whose, event))
        })
        .await
        .map_err(|error| format!("reading it: {error}"))?;
        match read {
            Ok(read) => return Ok(read),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted && overtaken < OVERTAKEN_MOST => {
                overtaken += 1;
                tokio::time::sleep(Duration::from_millis(100) * overtaken.min(10)).await;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                return Err(format!("{} changed each of the {OVERTAKEN_MOST} times it was read", path.display()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("{} is gone; Claude Code removes old sessions itself", path.display()));
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
}

/// What `hosted` would read, as cheaply as it can be told: the sessions the
/// resident has been, the last record the mod saw kept in each, each one's
/// record file by size and time of change, and the turns cut from it.
/// Claude Code only appends to a record and the World's cuts are files of
/// their own, so equal stamps mean `hosted` reads the same.
pub fn stamp(node: &Node, resident: &ResidentId, digest: &mut impl FnMut(&[u8])) {
    let root = exchange::sessions(node.world.path());
    let (sessions, problems) = sessions(node, resident);
    for problem in problems {
        digest(problem.as_bytes());
    }
    for Session { id, began, kept, .. } in sessions {
        let directory = root.join(&id);
        digest(format!("{id}|{began}|{kept:?}").as_bytes());
        let cuts = fastctx_platform::disk::metadata(directory.join("history-cuts.json"))
            .map(|meta| (meta.len(), meta.modified().ok()));
        digest(format!("{cuts:?}").as_bytes());
        let record = exchange::transcript(&directory).map(|path| {
            path.map(|path| {
                // With when the files of the agents it started last came or
                // went: what they tell is part of what the record shows
                // (`subagents`).
                let agents = claude::subagents::directory(&path)
                    .and_then(|agents| fastctx_platform::disk::metadata(agents).ok())
                    .and_then(|meta| meta.modified().ok());
                (fastctx_platform::disk::metadata(&path).map(|meta| (meta.len(), meta.modified().ok())), agents, path)
            })
        });
        digest(format!("{record:?}").as_bytes());
    }
}

/// The record Claude Code keeps of the agent `agent` one of the resident's
/// sessions started (`claude::subagents`): none where no session of the
/// resident's has one by that id.
pub(crate) fn agent_record(node: &Node, resident: &ResidentId, agent: &str) -> Option<PathBuf> {
    let root = exchange::sessions(node.world.path());
    let (sessions, _) = sessions(node, resident);
    sessions.into_iter().rev().find_map(|session| {
        let path = exchange::transcript(&root.join(&session.id)).ok().flatten()?;
        claude::subagents::record(&path, agent).filter(|record| fastctx_platform::disk::is_file(record))
    })
}

/// CLI forking reads the saved conversation. Wait for the last message
/// already observed by the mod before asking the CLI to copy that file.
pub(super) async fn wait_stored(node: &Arc<Node>, session: &str) -> Result<(), String> {
    let (kept, process) = {
        let state = node.claude.state.lock().expect("never poisoned");
        (
            state.stored.get::<String>(session),
            exchange::registration(&exchange::sessions(node.world.path()).join(session))
                .map_err(|error| error.to_string())?
                .and_then(|registration| registration.process),
        )
    };
    if kept.is_none() {
        return Ok(());
    }
    let directory = exchange::sessions(node.world.path()).join(session);
    let (read, whole) = read_whole(node, session, directory, kept, process, HashSet::new()).await?;
    if !whole {
        return Err(format!(
            "Claude Code did not write the last of what it kept to its record within {} s",
            WRITTEN_WITHIN.as_secs()
        ));
    }
    if read.problems.is_empty() { Ok(()) } else { Err(read.problems.join("; ")) }
}

/// A session the resident has been, as its mod registered it.
struct Session {
    id: String,
    began: Timestamp,
    process: Option<ProcessIdentity>,
    /// The last record its mod saw Claude Code keep, in this run of the node.
    kept: Option<String>,
}

/// The sessions the resident has been, the oldest first: found on disk
/// without the Claude state, which is asked only what its mod last saw.
/// Which directories are its own is kept in memory (`aliases`): only those
/// are read.
fn sessions(node: &Node, resident: &ResidentId) -> (Vec<Session>, Vec<String>) {
    let mut sessions = Vec::new();
    let (directories, mut problems) = node.claude.aliases.of(&exchange::sessions(node.world.path()), resident);
    for directory in directories {
        match exchange::registration(&directory) {
            Ok(Some(registration)) => sessions.push(Session {
                kept: None,
                id: registration.session,
                began: registration.at.unwrap_or_default(),
                process: registration.process,
            }),
            Ok(None) => {}
            Err(error) => problems.push(format!("reading {}: {error}", directory.display())),
        }
    }
    let state = node.claude.state.lock().expect("never poisoned");
    for session in &mut sessions {
        session.kept = state.stored.get(&session.id);
    }
    drop(state);
    sessions.sort_by_key(|session| session.began);
    (sessions, problems)
}

/// Reads a session's record once it holds `kept`, and whether it did.
/// Claude Code writes its file a while after it keeps a record, with no
/// bound on how long and no sign when it has (2.1.289): what was kept just
/// before is waited for, read again at each change to the file, until it is
/// there, or a record its mod saw kept since is, as the file holds them in
/// the order they were kept; until the session's process has ended; or for
/// `WRITTEN_WITHIN` at most, as a record the mod saw may reach the file
/// under another UUID or never.
async fn read_whole(
    node: &Arc<Node>,
    session: &str,
    directory: PathBuf,
    kept: Option<String>,
    process: Option<ProcessIdentity>,
    earlier: HashSet<String>,
) -> Result<(transcript::Transcript, bool), String> {
    let (read, whole) = read_kept(node, session, directory, kept, process, earlier).await?;
    // Stopped before a line larger than World reads (§28.49): the rows before
    // it are shown, with why it goes no further, but the reading is not the
    // record whole — nothing is taken away by it, and it is read again.
    let whole = whole && read.refused.is_none();
    Ok((read, whole))
}

/// As `read_whole` reads, whether the record held what was kept.
async fn read_kept(
    node: &Arc<Node>,
    session: &str,
    directory: PathBuf,
    kept: Option<String>,
    process: Option<ProcessIdentity>,
    earlier: HashSet<String>,
) -> Result<(transcript::Transcript, bool), String> {
    let (cuts, path) = {
        let directory = directory.clone();
        fastctx_platform::task::blocking(move || {
            let cuts = super::rewind::cuts(&directory).map_err(|error| error.to_string())?;
            let path = exchange::transcript(&directory)
                .map_err(|error| format!("where it is kept could not be read: {error}"))?
                .ok_or("the World's plugin never said where Claude Code keeps it")?;
            Ok::<_, String>((cuts, path))
        })
        .await?
    };
    let Some(kept) = kept else { return read_now(path, cuts, earlier).await.map(|read| (read, true)) };
    let _waiting = fastctx_platform::doing::waiting_outside(
        format!("Claude Code to write {kept} to session {session}'s record"),
        WRITTEN_WITHIN,
    );
    let gave_up = tokio::time::sleep(WRITTEN_WITHIN);
    tokio::pin!(gave_up);
    // Watched before it is read, so no change between the two goes unseen.
    // The record is on disk whether or not its folder can be watched: one
    // that cannot is read again every two seconds instead.
    let (changed, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
        if crate::file_events::may_change(&event) {
            let _ = changed.send(());
        }
    });
    let folder = path.parent().ok_or_else(|| format!("{} has no folder", path.display()))?;
    let watched = watcher.watch(folder, notify::RecursiveMode::NonRecursive).is_ok();
    let mut ended: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>> = match process {
        Some(process) => Box::pin(async move { fastctx_platform::host_process::exited(&process).await }),
        None => Box::pin(std::future::pending()),
    };
    loop {
        let read = read_now(path.clone(), cuts.clone(), earlier.clone()).await?;
        let latest = node.claude.state.lock().expect("never poisoned").stored.get::<String>(session);
        if read.uuids.contains(&kept) || latest.is_some_and(|latest| read.uuids.contains(&latest)) {
            return Ok((read, true));
        }
        tokio::select! {
            Some(()) = changes.recv() => {}
            () = tokio::time::sleep(Duration::from_secs(2)), if !watched => {}
            () = &mut gave_up => {
                // Waited for in vain: the readings after this take the file as
                // it is, until the mod sees another record kept.
                {
                    let state = node.claude.state.lock().expect("never poisoned");
                    if state.stored.get::<String>(session).as_deref() == Some(kept.as_str()) {
                        state.stored.remove(&session);
                    }
                }
                node.log.line(format_args!(
                    "Claude Code did not write {kept} to session {session}'s record within {} s; it is read as its file holds it",
                    WRITTEN_WITHIN.as_secs()
                ));
                return Ok((read, false));
            }
            result = &mut ended => {
                result.map_err(|error| format!("observing Claude's process: {error}"))?;
                let mut read = read_now(path.clone(), cuts.clone(), earlier.clone()).await?;
                if !read.uuids.contains(&kept) {
                    read.problems.push("its process ended before its file held the last of what it kept".to_string());
                }
                return Ok((read, true));
            }
        }
    }
}

/// The record as the file holds it now. A session at work adds to its file
/// as it goes; a read it overtook is read again a little later, until one
/// sees it whole, `OVERTAKEN_MOST` times at most.
async fn read_now(
    path: PathBuf,
    cuts: Vec<transcript::HistoryCut>,
    earlier: HashSet<String>,
) -> Result<transcript::Transcript, String> {
    let mut overtaken = 0;
    loop {
        let reading = path.clone();
        let cuts = cuts.clone();
        let earlier = earlier.clone();
        let read =
            fastctx_platform::task::spawn_blocking(move || transcript::read_continuing(&reading, &cuts, &earlier))
                .await
                .map_err(|error| format!("reading it: {error}"))?;
        match read {
            Ok(read) => return Ok(read),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted && overtaken < OVERTAKEN_MOST => {
                overtaken += 1;
                // The writing that overtook it comes in bursts: let one end.
                tokio::time::sleep(Duration::from_millis(100) * overtaken.min(10)).await;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                return Err(format!("{} changed each of the {OVERTAKEN_MOST} times it was read", path.display()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("{} is gone; Claude Code removes old sessions itself", path.display()));
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
}

/// An item the host deliberately keeps out of its conversation.
fn hidden(event: &SessionEvent) -> bool {
    matches!(&event.happened, Happened::Finished { item, .. } if matches!(item.what, fastctx_types::session::What::Hidden { .. }))
}

/// What the timeline keeps as it has of a record stopped short (§28.49,
/// `Span::Added`): each event `shown` of it as the timeline will know it
/// (`names::frozen`), words an envelope carries also as restored, by their
/// delivery (`talk::assemble`). Not a note the World adds of how it read
/// the record (that it stopped where it did): it is told as it is.
pub(crate) fn freeze(frozen: &mut HashSet<String>, shown: &[SessionEvent]) {
    for event in shown {
        if matches!(event.happened, Happened::Health { .. }) {
            continue;
        }
        frozen.insert(crate::timeline::names::frozen(event));
        if matches!(event.happened, Happened::Heard { .. }) {
            let restored = fastctx_types::session::attachments::restore(event.clone(), &fastctx_types::session::attachments::NoReceipts);
            frozen.extend(restored.iter().map(crate::timeline::names::frozen));
        }
    }
}

pub(crate) fn note(at: Timestamp, text: String) -> SessionEvent {
    SessionEvent { at, happened: Happened::Health { turn: None, health: Health { kind: HealthKind::Warning, text } } }
}

#[cfg(test)]
mod frozen {
    use super::*;

    /// Words an envelope carries are kept under the name the timeline gives
    /// them once restored, not only under the host's.
    #[test]
    fn words_in_an_envelope_are_kept_as_restored() {
        let file = fastctx_types::attachments::Attachment {
            id: "desk-f-0001".parse().unwrap(),
            owner: fastctx_types::Address { machine: "desk".parse().unwrap(), resident: "claude-a0a0".parse().unwrap() },
            conversation: None,
            town: None,
            path: "Pasted text.txt".into(),
            file: fastctx_types::attachments::UploadFile {
                name: "Pasted text.txt".into(),
                media_type: "text/plain".into(),
                size: 40_000,
                sha256: "0".repeat(64),
            },
        };
        let attached = fastctx_types::session::attachments::AttachedMessage {
            delivery: "desk-j-k3x9a2".parse().unwrap(),
            from: "desk".parse().unwrap(),
            attachments: vec![file],
        };
        let message = fastctx_types::session::UserMessage {
            attachments: Vec::new(),
            images: Vec::new(),
            id: "native-1".into(),
            client_id: None,
            content: Vec::new(),
            hook_fragments: Vec::new(),
            presentation: serde_json::Value::Null,
            detail: serde_json::Value::Null,
        };
        let event = SessionEvent {
            at: Timestamp::from_second(1_800_000_000).unwrap(),
            happened: Happened::Heard {
                delivery: None,
                turn: None,
                from: None,
                text: attached.render("the line beside it"),
                message: Some(Box::new(message)),
                notice: None,
            },
        };
        let mut frozen = HashSet::new();
        freeze(&mut frozen, std::slice::from_ref(&event));
        let placed = crate::talk::merged(Vec::new(), Some(vec![event]), "claude", &fastctx_types::session::attachments::NoReceipts);
        assert!(!placed.is_empty());
        for event in &placed {
            let known = crate::timeline::names::frozen(event);
            assert!(known.starts_with("delivery/"), "{known}");
            assert!(frozen.contains(&known), "{known} not in {frozen:?}");
        }
    }
}
