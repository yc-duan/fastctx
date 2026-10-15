//! The folders the user chose on this machine for logins kept below them
//! (`world-agent-logins.md`, 世界怎样找到它们). A folder that is a login
//! itself is taken up as a candidate; any other is kept, and everything
//! below it is searched by the names in it, on a thread at the lowest
//! priority, which the user can stop. A search runs when the folder is
//! chosen and again with every scan, and what it finds shows as it is met.
//! What it finds only looks like a login: no one is asked who is logged in
//! there until the user takes one up.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fastctx_hosts::inventory::{traces, walk};
use fastctx_types::AgentName;
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{Folder, FolderChange, FolderSearch, FoundIn, MachineLogins, Stage};
use jiff::Timestamp;

use super::kept::{self, KeptSearch};
use super::{AGENTS, Context};
use crate::Node;

/// The searches under way, by the folder's key.
#[derive(Default)]
pub(super) struct Searches(Mutex<HashMap<String, Arc<Searching>>>);

/// One folder's search under way.
#[derive(Default)]
struct Searching {
    stop: AtomicBool,
    counters: walk::Counters,
    /// What it found so far, in the order met.
    found: Mutex<Vec<FoundIn>>,
}

impl Searches {
    fn get(&self, key: &str) -> Option<Arc<Searching>> {
        self.0.lock().expect("never poisoned").get(key).cloned()
    }

    /// Whether `searching` is the search of `key` under way: one stopped
    /// and replaced by a newer search of the same folder no longer is.
    fn current(&self, key: &str, searching: &Arc<Searching>) -> bool {
        self.get(key).is_some_and(|current| Arc::ptr_eq(&current, searching))
    }
}

/// What the user does with the folder `dir`.
pub(super) async fn change(node: &Arc<Node>, dir: PathBuf, change: FolderChange) -> Result<MachineLogins, Failure> {
    match change {
        FolderChange::Choose => choose(node, dir).await,
        FolderChange::Stop => {
            if let Some(searching) = node.logins.searches.get(&walk::key(&dir)) {
                searching.stop.store(true, Ordering::Relaxed);
            }
            super::here(node).await
        }
        FolderChange::Forget => forget(node, dir).await,
    }
}

/// Chooses `dir` for logins: one that is a login itself is taken up as a
/// candidate of its agent; any other is kept and searched now.
async fn choose(node: &Arc<Node>, dir: PathBuf) -> Result<MachineLogins, Failure> {
    if !dir.is_absolute() {
        return Err(Failure::not_run(format!("{} is not an absolute path", dir.display())));
    }
    let agents = {
        let dir = dir.clone();
        let machine = node.machine();
        tokio::task::spawn_blocking(move || {
            if !dir.is_dir() {
                return Err(Failure::not_run(format!("{} is not a folder on {machine}", dir.display())));
            }
            Ok(logins_of(&dir))
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    };
    if !agents.is_empty() {
        for agent in agents {
            let agent = AgentName::try_from(agent.to_string()).map_err(|error| Failure::new(error.to_string()))?;
            super::take_up(node, agent, dir.clone()).await?;
        }
        return super::here(node).await;
    }
    {
        let (node, dir) = (node.clone(), dir.clone());
        tokio::task::spawn_blocking(move || kept::add_folder(&node, &dir))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("keeping the folder: {error}")))?;
    }
    search(node, dir);
    super::here(node).await
}

/// No longer searches `dir`: its search stops, and what it found goes.
async fn forget(node: &Arc<Node>, dir: PathBuf) -> Result<MachineLogins, Failure> {
    if let Some(searching) = node.logins.searches.0.lock().expect("never poisoned").remove(&walk::key(&dir)) {
        searching.stop.store(true, Ordering::Relaxed);
    }
    {
        let (node, dir) = (node.clone(), dir.clone());
        tokio::task::spawn_blocking(move || {
            kept::remove_folder(&node, &dir)?;
            kept::change(&node, |kept| kept.folders.retain(|one| !crate::homes::same_dir(&one.dir, &dir)))
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("forgetting the folder: {error}")))?;
    }
    super::here(node).await
}

/// Searches every chosen folder again; a search under way goes on.
pub(super) fn search_all(node: &Arc<Node>) -> Result<(), Failure> {
    let folders = kept::folders(node).map_err(|error| Failure::new(format!("reading the chosen folders: {error}")))?;
    for dir in folders {
        search(node, dir);
    }
    Ok(())
}

/// Stops every search under way.
pub(super) fn stop_all(node: &Node) {
    for searching in node.logins.searches.0.lock().expect("never poisoned").values() {
        searching.stop.store(true, Ordering::Relaxed);
    }
}

/// The agents `dir` is a login of: named and kept like the agent's own, or
/// holding what holds a login wherever it is (`traces`).
fn logins_of(dir: &Path) -> Vec<&'static str> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let entries: Vec<traces::Entry> = read
        .flatten()
        .map(|entry| {
            (entry.file_name().to_string_lossy().into_owned(), entry.file_type().is_ok_and(|kind| kind.is_dir()))
        })
        .collect();
    let name = dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    AGENTS
        .into_iter()
        .filter(|agent| traces::usual(agent, &name, &entries) || traces::elsewhere(agent, &entries).is_some())
        .collect()
}

/// Starts the search of `dir`, unless one runs; one told to stop is
/// replaced, and what it would keep as it ends is no longer kept.
fn search(node: &Arc<Node>, dir: PathBuf) {
    let key = walk::key(&dir);
    let searching = {
        let mut searches = node.logins.searches.0.lock().expect("never poisoned");
        if searches.get(&key).is_some_and(|running| !running.stop.load(Ordering::Relaxed)) {
            return;
        }
        let searching = Arc::new(Searching::default());
        searches.insert(key.clone(), searching.clone());
        searching
    };
    let spawned = std::thread::Builder::new().name("agents' logins search".into()).spawn({
        let (node, key, searching) = (node.clone(), key.clone(), searching.clone());
        move || {
            if let Err(failure) = run(&node, &dir, &key, &searching) {
                node.log.line(format_args!("searching {} for agents' logins: {}", dir.display(), failure.message));
            }
            let mut searches = node.logins.searches.0.lock().expect("never poisoned");
            if searches.get(&key).is_some_and(|current| Arc::ptr_eq(current, &searching)) {
                searches.remove(&key);
            }
        }
    });
    if let Err(error) = spawned {
        node.logins.searches.0.lock().expect("never poisoned").remove(&key);
        node.log.line(format_args!("starting a search for agents' logins: {error}"));
    }
}

/// One search of `dir`, kept as it starts, so that a node that stops during
/// it leaves a record saying so, and as it ends. A search stopped because
/// its folder was forgotten, or replaced by a newer one, keeps nothing.
fn run(node: &Arc<Node>, dir: &Path, key: &str, searching: &Arc<Searching>) -> Result<(), Failure> {
    let keep = |search: FolderSearch| {
        kept::change(node, |kept| {
            if !node.logins.searches.current(key, searching) {
                return;
            }
            kept.folders.retain(|one| !crate::homes::same_dir(&one.dir, dir));
            kept.folders.push(KeptSearch { dir: dir.to_path_buf(), search });
        })
        .map_err(|error| Failure::new(format!("keeping the search: {error}")))
    };
    keep(FolderSearch { stage: Stage::Running, directories: 0, unreadable: 0, found: Vec::new() })?;
    let walked = (|| {
        fastctx_platform::disks::lower_this_thread()
            .map_err(|error| format!("the search could not be put in the background: {error}"))?;
        // The World's own directory, and the logins known already, are not
        // searched into.
        let context = Context::of(node).map_err(|failure| failure.message)?;
        let known = kept::read(node).map_err(|error| format!("reading the agents' logins: {error}"))?;
        let mut skip: HashSet<String> = HashSet::from([walk::key(&node.root)]);
        for agent in AGENTS {
            for (login, _) in super::dirs(node, &context, &known, agent).map_err(|failure| failure.message)? {
                skip.insert(walk::key(&login));
            }
        }
        // Nor is any place a filesystem is mounted inside the folder: one
        // mounted again inside itself would be searched twice.
        let mounted = fastctx_platform::disks::mount_points()
            .map_err(|error| format!("listing where filesystems are mounted: {error}"))?;
        skip.extend(mounted.iter().filter(|point| !crate::homes::same_dir(point, dir)).map(|point| walk::key(point)));
        walk::walk(dir, &skip, &searching.stop, &searching.counters, &mut |one| {
            if let Ok(path) = one.dir.strip_prefix(dir)
                && !path.as_os_str().is_empty()
                && let Ok(agent) = AgentName::try_from(one.agent.to_string())
            {
                searching.found.lock().expect("never poisoned").push(FoundIn {
                    agent,
                    path: path.to_path_buf(),
                    traces: one.traces,
                });
            }
        });
        Ok::<_, String>(())
    })();
    let at = Timestamp::now();
    let mut found = searching.found.lock().expect("never poisoned").clone();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    let stage = match walked {
        Ok(()) if searching.stop.load(Ordering::Relaxed) => Stage::Stopped { at },
        Ok(()) => Stage::Done { at },
        Err(reason) => Stage::Failed { at, reason },
    };
    keep(FolderSearch {
        stage,
        directories: searching.counters.directories.load(Ordering::Relaxed),
        unreadable: searching.counters.unreadable.load(Ordering::Relaxed),
        found,
    })
}

/// The chosen folders as the page shows them: each with its search, its
/// progress and what it found so far while it runs, and only what the World
/// does not know already. What lies below another chosen folder inside this
/// one shows under that one alone. A search the record says runs, with none
/// running, was cut short when the node stopped.
pub(super) fn shown(node: &Node, kept: &kept::Kept, chosen: &[PathBuf], known: &HashSet<String>) -> Vec<Folder> {
    let keys: Vec<String> = chosen.iter().map(|dir| walk::key(dir)).collect();
    chosen
        .iter()
        .zip(&keys)
        .map(|(dir, key)| {
            let last =
                kept.folders.iter().find(|one| crate::homes::same_dir(&one.dir, dir)).map(|one| one.search.clone());
            let mut search = match (node.logins.searches.get(key), last) {
                (Some(running), _) => {
                    let mut found = running.found.lock().expect("never poisoned").clone();
                    found.sort_by(|a, b| a.path.cmp(&b.path));
                    // One told to stop ends with the directory it is
                    // listing: it is as good as stopped.
                    let stopped = running.stop.load(Ordering::Relaxed);
                    Some(FolderSearch {
                        stage: if stopped { Stage::Stopped { at: Timestamp::now() } } else { Stage::Running },
                        directories: running.counters.directories.load(Ordering::Relaxed),
                        unreadable: running.counters.unreadable.load(Ordering::Relaxed),
                        found,
                    })
                }
                (None, Some(mut last)) => {
                    if last.stage == Stage::Running {
                        last.stage =
                            Stage::Failed { at: Timestamp::now(), reason: "the node stopped during it".into() };
                    }
                    Some(last)
                }
                (None, None) => None,
            };
            if let Some(search) = &mut search {
                let inner: Vec<&String> = keys.iter().filter(|other| below(other, key)).collect();
                search.found.retain(|one| {
                    let path = walk::key(&dir.join(&one.path));
                    !known.contains(&path) && !inner.iter().any(|inner| below(&path, inner))
                });
            }
            Folder { dir: dir.clone(), search }
        })
        .collect()
}

/// Whether the path keyed `path` lies below the directory keyed `dir`.
pub(super) fn below(path: &str, dir: &str) -> bool {
    let separated = dir.ends_with(['\\', '/']);
    path.len() > dir.len()
        && path.starts_with(dir)
        && (separated || path[dir.len()..].starts_with(std::path::MAIN_SEPARATOR))
}
