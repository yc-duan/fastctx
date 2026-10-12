//! A scan of this machine for its agents, only when the user asks
//! (`world-agent-logins.md`, 世界怎样找到它们): first the usual places, the
//! agents' programs where their installers put them and each login there
//! asked who it is, which is answered at once; then a walk of every local
//! fixed disk by names alone, on threads at the lowest priority, one for
//! each disk, which the user can stop at any time. What the walk finds only
//! looks like a login: no one is asked who is logged in there until the
//! user takes one up.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fastctx_hosts::inventory::{self, traces, walk};
use fastctx_types::agents::Fact;
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{DiskProgress, DiskWalk, Found, LooksLike, MachineLogins, Scan, Stage};
use jiff::Timestamp;

use super::kept::{self, KeptLimits, KeptLogin};
use super::{AGENTS, Context, identify};
use crate::Node;

/// A scan under way.
pub(crate) struct Running {
    started_at: Timestamp,
    stop: AtomicBool,
    /// Each disk's walk, once it began.
    disks: Mutex<Vec<(PathBuf, Arc<walk::Counters>)>>,
}

impl Running {
    /// Writes how far the walk has got into the kept record of the scan.
    pub(super) fn progress(&self, scan: &mut Scan) {
        if scan.started_at != self.started_at {
            return;
        }
        if let Some(walk) = &mut scan.disks {
            walk.disks = self
                .disks
                .lock()
                .expect("never poisoned")
                .iter()
                .map(|(root, counters)| DiskProgress {
                    root: root.clone(),
                    directories: counters.directories.load(Ordering::Relaxed),
                    unreadable: counters.unreadable.load(Ordering::Relaxed),
                    done: counters.done.load(Ordering::Acquire),
                })
                .collect();
        }
    }
}

/// Scans this machine, answering once the usual places are done while the
/// walk of every disk goes on; with `stop`, stops that walk. Asked while
/// one runs, it answers how that one is going.
pub(crate) async fn scan(node: &Arc<Node>, stop: bool) -> Result<MachineLogins, Failure> {
    let running = node.logins.scan.lock().expect("never poisoned").clone();
    if stop || running.is_some() {
        if stop && let Some(running) = &running {
            running.stop.store(true, Ordering::Relaxed);
        }
        return super::here(node).await;
    }
    let started_at = Timestamp::now();
    let running = Arc::new(Running { started_at, stop: AtomicBool::new(false), disks: Mutex::new(Vec::new()) });
    let began = {
        let mut slot = node.logins.scan.lock().expect("never poisoned");
        let free = slot.is_none();
        if free {
            *slot = Some(running.clone());
        }
        free
    };
    if !began {
        return super::here(node).await;
    }
    let usual = usual(node, &running).await;
    if let Err(failure) = &usual {
        // The record says how it ended before the scan is no longer running,
        // so no reader sees it running in the record and nowhere else.
        let failed = Stage::Failed { at: Timestamp::now(), reason: failure.message.clone() };
        let kept = blocking(node, move |node| {
            kept::change(node, |kept| {
                if let Some(scan) = &mut kept.scan
                    && scan.started_at == started_at
                {
                    scan.usual = failed;
                }
            })
            .map_err(|error| Failure::new(format!("keeping the scan: {error}")))
        })
        .await;
        node.logins.scan.lock().expect("never poisoned").take();
        kept?;
        return Err(failure.clone());
    }
    let walking = node.clone();
    tokio::spawn(async move {
        let node = walking.clone();
        let outcome = tokio::task::spawn_blocking(move || disks(&walking, &running)).await;
        let problem = match outcome {
            Ok(Ok(())) => None,
            Ok(Err(failure)) => Some(failure.message),
            Err(error) => Some(error.to_string()),
        };
        if let Some(problem) = problem {
            node.log.line(format_args!("walking the disks for agents' logins: {problem}"));
            let kept = kept::change(&node, |kept| {
                if let Some(walk) = kept.scan.as_mut().and_then(|scan| scan.disks.as_mut()) {
                    walk.stage = Stage::Failed { at: Timestamp::now(), reason: problem.clone() };
                }
            });
            if let Err(error) = kept {
                node.log.line(format_args!("keeping the scan: {error}"));
            }
        }
        node.logins.scan.lock().expect("never poisoned").take();
    });
    super::here(node).await
}

async fn blocking<T: Send + 'static>(
    node: &Arc<Node>,
    work: impl FnOnce(&Node) -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    let node = node.clone();
    tokio::task::spawn_blocking(move || work(&node)).await.map_err(|error| Failure::new(error.to_string()))?
}

/// The usual places: each agent's default login, the directories directly
/// in the user's home named and kept like the agent's, those the user
/// added and those the World's tools are in, each asked who it is; and the
/// agents' programs.
async fn usual(node: &Arc<Node>, running: &Running) -> Result<(), Failure> {
    let started_at = running.started_at;
    let (context, kept, programs, candidates) = blocking(node, move |node| {
        let context = Context::of(node)?;
        kept::change(node, |kept| {
            kept.scan = Some(Scan { started_at, usual: Stage::Running, disks: None, unreadable: Vec::new() });
        })
        .map_err(|error| Failure::new(format!("keeping the scan: {error}")))?;
        let kept = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
        let programs = inventory::find(&inventory::Where {
            user_home: &context.user_home,
            env: &context.env,
            codex_home: &context.codex_default,
            claude_home: &context.claude_default,
        });
        let mut unreadable = programs.unreadable.clone();
        let mut candidates = Vec::new();
        for agent in AGENTS {
            let mut dirs = super::dirs(node, &context, &kept, agent)?;
            // A scan finds afresh what is in the usual places: what it found
            // before and is gone is not carried on.
            dirs.retain(|(dir, found)| *found != Found::Home || dir.is_dir());
            for dir in in_home(&context.user_home, agent, &mut unreadable) {
                if !dirs.iter().any(|(known, _)| crate::homes::same_dir(known, &dir)) {
                    dirs.push((dir, Found::Home));
                }
            }
            candidates.push((agent, dirs));
        }
        let mut programs = programs;
        programs.unreadable = unreadable;
        Ok((context, kept, programs, candidates))
    })
    .await?;
    // Each login is asked at once: a slow one does not hold the others.
    let mut asking = Vec::new();
    for (agent, dirs) in candidates {
        for (dir, found) in dirs {
            let node = node.clone();
            let claude_programs = programs.claude.clone();
            let default = crate::homes::same_dir(&dir, context.default_of(agent));
            asking.push(tokio::spawn(async move {
                let asked = if agent == "codex" {
                    identify::codex(&node, &dir).await
                } else {
                    let path = dir.clone();
                    match tokio::task::spawn_blocking(move || identify::claude(&path, default, &claude_programs)).await
                    {
                        Ok(Ok(asked)) => asked,
                        Ok(Err(failure)) => failed(failure.message),
                        Err(error) => failed(error.to_string()),
                    }
                };
                (agent, dir, found, asked)
            }));
        }
    }
    let mut answers = Vec::new();
    for asking in asking {
        answers.push(asking.await.map_err(|error| Failure::new(format!("asking a login: {error}")))?);
    }
    drop(kept);
    blocking(node, move |node| {
        kept::change(node, |kept| {
            kept.codex.programs = programs.codex;
            kept.claude.programs = programs.claude;
            kept.codex.logins.clear();
            kept.claude.logins.clear();
            for (agent, dir, found, asked) in answers {
                if let Some((at, answer)) = asked.limits {
                    let buckets = fastctx_hosts::codex::buckets(&answer);
                    let limits = &mut kept.codex.limits;
                    limits.retain(|known| !crate::homes::same_dir(&known.dir, &dir));
                    limits.push(KeptLimits { dir: dir.clone(), at, buckets });
                }
                kept.of_mut(agent).logins.push(KeptLogin {
                    dir,
                    found,
                    identified: asked.identified,
                    launcher: asked.launcher,
                    models: asked.models,
                });
            }
            if let Some(scan) = &mut kept.scan
                && scan.started_at == started_at
            {
                scan.usual = Stage::Done { at: Timestamp::now() };
                scan.unreadable = programs.unreadable;
                scan.disks = Some(DiskWalk { stage: Stage::Running, disks: Vec::new() });
            }
        })
        .map_err(|error| Failure::new(format!("keeping the scan: {error}")))
    })
    .await
}

fn failed(reason: String) -> identify::Asked {
    identify::Asked {
        identified: fastctx_types::logins::Identified { at: Timestamp::now(), who: Fact::failed(reason) },
        launcher: None,
        models: Vec::new(),
        limits: None,
    }
}

/// The directories directly in the user's home that are `agent`'s logins
/// by the usual rule, by the names of their entries.
fn in_home(home: &Path, agent: &str, unreadable: &mut Vec<String>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(home) {
        Ok(entries) => entries,
        Err(error) => {
            unreadable.push(format!("{}: {error}", home.display()));
            return found;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let names = match std::fs::read_dir(entry.path()) {
            Ok(names) => names
                .flatten()
                .map(|inner| {
                    (
                        inner.file_name().to_string_lossy().into_owned(),
                        inner.file_type().is_ok_and(|kind| kind.is_dir()),
                    )
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                if traces::AGENTS.iter().any(|(_, prefix)| name.starts_with(prefix)) {
                    unreadable.push(format!("{}: {error}", entry.path().display()));
                }
                continue;
            }
        };
        if traces::usual(agent, &name, &names) {
            found.push(entry.path());
        }
    }
    found.sort();
    found
}

/// The walk of every local fixed disk, a thread each, lowered to the
/// background; its findings kept once every disk is done or the user
/// stopped it.
fn disks(node: &Arc<Node>, running: &Arc<Running>) -> Result<(), Failure> {
    let roots = fastctx_platform::disks::fixed_disks()
        .map_err(|error| Failure::new(format!("listing the local fixed disks: {error}")))?;
    let context = Context::of(node)?;
    let kept = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
    // The World's own directory, and the logins known already, are not
    // walked into.
    let mut skip: HashSet<String> = HashSet::from([walk::key(&node.root)]);
    // Nor is any place a filesystem is mounted again: each disk is walked
    // from its own root, once.
    let mounted = fastctx_platform::disks::mount_points()
        .map_err(|error| Failure::new(format!("listing where filesystems are mounted: {error}")))?;
    for point in mounted.iter().filter(|point| !roots.contains(point)) {
        skip.insert(walk::key(point));
    }
    for agent in AGENTS {
        for (dir, _) in super::dirs(node, &context, &kept, agent)? {
            skip.insert(walk::key(&dir));
        }
    }
    let skip = Arc::new(skip);
    let found: Arc<Mutex<Vec<walk::Found>>> = Arc::default();
    let problems: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut threads = Vec::new();
    for root in roots {
        let counters = Arc::new(walk::Counters::default());
        running.disks.lock().expect("never poisoned").push((root.clone(), counters.clone()));
        let (running, skip, found, told, walked) =
            (running.clone(), skip.clone(), found.clone(), problems.clone(), root.clone());
        let thread = std::thread::Builder::new().name("agents' logins walk".into()).spawn(move || {
            let (root, problems) = (walked, told);
            if let Err(error) = fastctx_platform::disks::lower_this_thread() {
                problems.lock().expect("never poisoned").push(format!(
                    "{}: not walked, as its walk could not be put in the background: {error}",
                    root.display()
                ));
                counters.done.store(true, Ordering::Release);
                return;
            }
            walk::walk(&root, &skip, &running.stop, &counters, &mut |one| {
                found.lock().expect("never poisoned").push(one);
            });
        });
        match thread {
            Ok(thread) => threads.push(thread),
            Err(error) => {
                problems.lock().expect("never poisoned").push(format!("{}: not walked: {error}", root.display()))
            }
        }
    }
    for thread in threads {
        if thread.join().is_err() {
            problems.lock().expect("never poisoned").push("a disk's walk panicked".into());
        }
    }
    let stopped = running.stop.load(Ordering::Relaxed);
    let found = std::mem::take(&mut *found.lock().expect("never poisoned"));
    let problems = std::mem::take(&mut *problems.lock().expect("never poisoned"));
    let progress: Vec<DiskProgress> = running
        .disks
        .lock()
        .expect("never poisoned")
        .iter()
        .map(|(root, counters)| DiskProgress {
            root: root.clone(),
            directories: counters.directories.load(Ordering::Relaxed),
            unreadable: counters.unreadable.load(Ordering::Relaxed),
            done: counters.done.load(Ordering::Acquire),
        })
        .collect();
    let started_at = running.started_at;
    kept::change(node, |kept| {
        for agent in AGENTS {
            let mut elsewhere: Vec<LooksLike> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            for one in found.iter().filter(|one| one.agent == agent) {
                if seen.insert(walk::key(&one.dir)) {
                    elsewhere.push(LooksLike { dir: one.dir.clone(), traces: one.traces.clone() });
                }
            }
            // A walk the user stopped did not reach everywhere: what an
            // earlier walk found there, and is still a directory, is still
            // the best word on it.
            if stopped {
                for earlier in &kept.of(agent).elsewhere {
                    if earlier.dir.is_dir() && seen.insert(walk::key(&earlier.dir)) {
                        elsewhere.push(earlier.clone());
                    }
                }
            }
            elsewhere.sort_by(|a, b| a.dir.cmp(&b.dir));
            kept.of_mut(agent).elsewhere = elsewhere;
        }
        if let Some(scan) = &mut kept.scan
            && scan.started_at == started_at
        {
            let at = Timestamp::now();
            scan.disks = Some(DiskWalk {
                stage: if stopped { Stage::Stopped { at } } else { Stage::Done { at } },
                disks: progress,
            });
            scan.unreadable.extend(problems);
        }
    })
    .map_err(|error| Failure::new(format!("keeping the scan: {error}")))
}
