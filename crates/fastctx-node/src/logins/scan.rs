//! A scan of this machine for its agents, only when the user asks
//! (`world-agent-logins.md`, 世界怎样找到它们): the usual places, the
//! agents' programs where their installers put them and each login there
//! asked who it is, which takes seconds; then every folder the user chose
//! is searched again in the background (`folders`). With `stop`, those
//! searches stop.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use fastctx_hosts::inventory::{self, traces};
use fastctx_types::agents::Fact;
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{Found, MachineLogins, Scan, Stage};
use jiff::Timestamp;

use super::kept::{self, KeptLimits, KeptLogin};
use super::{AGENTS, Context, identify};
use crate::Node;

/// Scans this machine, answering once the usual places are done while the
/// chosen folders are searched; with `stop`, stops those searches. Asked
/// while a scan runs, it answers how that one is going. With `limits`,
/// each Codex login is asked its usage limits too. The scan goes on to its
/// end even when whoever asked for it is gone.
pub(crate) async fn scan(node: &Arc<Node>, stop: bool, limits: bool) -> Result<MachineLogins, Failure> {
    if stop {
        super::folders::stop_all(node);
        return super::here(node).await;
    }
    if node.logins.scanning.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return super::here(node).await;
    }
    let scanning = node.clone();
    let scanned = tokio::spawn(async move {
        let node = scanning;
        let scanned = scan_once(&node, limits).await;
        // What the scan finds is listed as this fastctx writes it.
        super::refresh::all(&node).await;
        node.logins.scanning.store(false, Ordering::Release);
        scanned
    })
    .await
    .map_err(|error| Failure::new(format!("scanning: {error}")));
    scanned??;
    super::here(node).await
}

/// The usual places, then the chosen folders' searches; the record says how
/// the scan ended before the scan is no longer running, so no reader sees
/// it running in the record and nowhere else.
async fn scan_once(node: &Arc<Node>, limits: bool) -> Result<(), Failure> {
    let started_at = Timestamp::now();
    if let Err(failure) = usual(node, started_at, limits).await {
        let failed = Stage::Failed { at: Timestamp::now(), reason: failure.message.clone() };
        blocking(node, move |node| {
            kept::change(node, |kept| {
                if let Some(scan) = &mut kept.scan
                    && scan.started_at == started_at
                {
                    scan.usual = failed;
                }
            })
            .map_err(|error| Failure::new(format!("keeping the scan: {error}")))
        })
        .await?;
        return Err(failure);
    }
    super::folders::search_all(node)
}

async fn blocking<T: Send + 'static>(
    node: &Arc<Node>,
    work: impl FnOnce(&Node) -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    let node = node.clone();
    tokio::task::spawn_blocking(move || work(&node)).await.map_err(|error| Failure::new(error.to_string()))?
}

/// The usual places: each agent's default login, the directories directly
/// in the user's home named and kept like the agent's, those the user took
/// up and those the World's tools are in, each asked who it is; and the
/// agents' programs.
async fn usual(node: &Arc<Node>, started_at: Timestamp, limits: bool) -> Result<(), Failure> {
    let (context, programs, candidates) = blocking(node, move |node| {
        let context = Context::of(node)?;
        kept::change(node, |kept| {
            kept.scan = Some(Scan { started_at, usual: Stage::Running, unreadable: Vec::new() });
        })
        .map_err(|error| Failure::new(format!("keeping the scan: {error}")))?;
        let kept = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
        let mut programs = inventory::find(&inventory::Where {
            user_home: &context.user_home,
            env: &context.env,
            codex_home: &context.codex_default,
            claude_home: &context.claude_default,
        });
        let mut candidates = Vec::new();
        for agent in AGENTS {
            let mut dirs = super::dirs(node, &context, &kept, agent)?;
            // A scan finds afresh what is in the usual places: what it found
            // before and is gone is not carried on.
            dirs.retain(|(dir, found)| *found != Found::Home || dir.is_dir());
            for dir in in_home(&context.user_home, agent, &mut programs.unreadable) {
                if !dirs.iter().any(|(known, _)| crate::homes::same_dir(known, &dir)) {
                    dirs.push((dir, Found::Home));
                }
            }
            candidates.push((agent, dirs));
        }
        Ok((context, programs, candidates))
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
                    identify::codex(&node, &dir, limits).await
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
