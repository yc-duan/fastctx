//! A Claude resident's history as Claude Code keeps it (`world-colleagues.md`,
//! 用户在任何地方和居民说话): each of its sessions read from Claude Code's
//! own record of it, the one host file the World reads (`world.md`), and
//! placed in the turns the World saw. The World's own record says who spoke
//! and how each turn ended; it keeps no copy of the conversation.

use std::collections::HashSet;

mod placement;
pub(super) use placement::place;
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
                    // Read before it held all its mod saw kept: read again next
                    // time, not kept.
                    unread |= !whole;
                    unsettled |= !whole;
                    // The title belongs to the session, so it takes the session's
                    // time: read again, the history reads the same.
                    if let Some(title) = read.title {
                        events.push(SessionEvent { at: began, happened: Happened::Titled { title } });
                    }
                    events.extend(read.events.into_iter().map(|mut event| {
                        crate::user_images::claude(node, resident, &mut event);
                        event
                    }));
                    if !read.problems.is_empty() {
                        let text = format!(
                            "{} part(s) of Claude Code's record of session {id} could not be read: {}",
                            read.problems.len(),
                            read.problems.join("; ")
                        );
                        events.push(note(began, text.clone()));
                        unavailable.push(text);
                    }
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
                (fastctx_platform::disk::metadata(&path).map(|meta| (meta.len(), meta.modified().ok())), path)
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
fn sessions(node: &Node, resident: &ResidentId) -> (Vec<Session>, Vec<String>) {
    let mut sessions = Vec::new();
    let mut problems = Vec::new();
    let scanned = (|| -> std::io::Result<()> {
        for entry in fastctx_platform::disk::read_dir(exchange::sessions(node.world.path()))? {
            let directory = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    problems.push(format!("reading a Claude session directory: {error}"));
                    continue;
                }
            };
            let read = (|| -> std::io::Result<Option<Session>> {
                let alias = match fastctx_platform::disk::read(directory.join("resident.json")) {
                    Ok(bytes) => serde_json::from_slice::<ResidentId>(&bytes).map_err(std::io::Error::other)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => return Err(error),
                };
                if &alias != resident {
                    return Ok(None);
                }
                if let Some(registration) = exchange::registration(&directory)? {
                    return Ok(Some(Session {
                        kept: None,
                        id: registration.session,
                        began: registration.at.unwrap_or_default(),
                        process: registration.process,
                    }));
                }
                Ok(None)
            })();
            match read {
                Ok(Some(session)) => sessions.push(session),
                Ok(None) => {}
                Err(error) => problems.push(format!("reading {}: {error}", directory.display())),
            }
        }
        Ok(())
    })();
    if let Err(error) = scanned {
        problems.push(format!("reading {resident}'s Claude sessions: {error}"));
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

fn note(at: Timestamp, text: String) -> SessionEvent {
    SessionEvent { at, happened: Happened::Health { turn: None, health: Health { kind: HealthKind::Warning, text } } }
}
