//! Session-scoped auxiliary files are separate from Claude's conversations.

use crate::{Action, Request, Step};
use fastctx_types::residents::About;
use fastctx_types::uninstall::Item;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization as _;

pub(crate) fn include_steps(request: &mut Request) -> anyhow::Result<()> {
    let world = fastctx_types::layout::WorldDir::under(&request.installation.root);
    let mut residents = BTreeMap::new();
    for file in crate::inventory::abouts(&world)? {
        if let Some(about) = crate::read::<About>(&file)?
            && about.world_started
            && about.host.agent.as_str() == "claude"
        {
            residents.insert(about.id.clone(), about);
        }
    }
    let mut homes = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    let mut jobs = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for about in residents.values() {
        if let Some(session) = &about.session {
            let home = config_home(about.home.as_deref().unwrap_or(&request.installation.claude_home))?;
            if uuid(session) {
                homes.entry(home.clone()).or_default().insert(session.clone());
                jobs.entry(home.join("jobs").join(&session[..8])).or_default().insert(session.clone());
            } else if session.len() == 8 && session.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                jobs.entry(home.join("jobs").join(session)).or_default();
            }
        }
    }
    // The mod records the full identity and its resident binding. About can
    // still hold the eight-character ID printed by --bg, including after clear.
    for entry in crate::entries(&world.path().join("claude"))? {
        let Some(session) = entry.file_name().to_str().filter(|name| uuid(name)).map(str::to_owned) else { continue };
        let Some(resident) = crate::read::<fastctx_types::ResidentId>(&entry.path().join("resident.json"))? else {
            continue;
        };
        let Some(about) = residents.get(&resident) else { continue };
        let home = config_home(about.home.as_deref().unwrap_or(&request.installation.claude_home))?;
        homes.entry(home.clone()).or_default().insert(session.clone());
        if let Some(short) =
            about.session.as_deref().filter(|id| id.len() == 8 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            jobs.entry(home.join("jobs").join(short)).or_default().insert(session);
        }
    }
    for (home, sessions) in homes {
        for session in sessions {
            for path in [
                home.join("session-env").join(&session),
                home.join("debug").join(format!("{session}.txt")),
                home.join("debug").join(format!("{session}.1.txt")),
                home.join("startup-perf").join(format!("{session}.txt")),
                home.join("startup-perf").join(format!("{session}.json")),
            ] {
                include(request, path, None, Vec::new())?;
            }
        }
    }
    for (job, sessions) in jobs {
        include(request, job.join("tmp"), Some(job), sessions.into_iter().collect())?;
    }
    Ok(())
}

fn include(request: &mut Request, path: PathBuf, job: Option<PathBuf>, sessions: Vec<String>) -> anyhow::Result<()> {
    if !path.try_exists()?
        || request
            .steps
            .iter()
            .any(|step| matches!(&step.action, Action::ClaudeSessionCache { path: existing, .. } if existing == &path))
    {
        return Ok(());
    }
    let location = path.display().to_string();
    if request.plan.keeps.iter().any(|item| item.location.as_deref() == Some(&location)) {
        return Ok(());
    }
    let action = Action::ClaudeSessionCache { path, job, sessions };
    let shared = request.plan.everything && !removable(&action)?;
    let item = Item {
        id: format!("claude-session-cache-{location}"),
        kind: if shared { "claude-shared-cache" } else { "claude-work-cache" }.into(),
        what: if shared {
            "Claude cache entry without exclusive World ownership"
        } else {
            "World-owned Claude Code session environment, diagnostic logs or temporary files"
        }
        .into(),
        location: Some(location),
        admin: false,
    };
    if shared {
        request.plan.keeps.push(item);
        return Ok(());
    }
    if request.plan.everything {
        let index = request
            .steps
            .iter()
            .position(|step| matches!(step.action, Action::Programs | Action::WorldData | Action::Delete(_)))
            .unwrap_or(request.steps.len());
        request.plan.steps.insert(index, item.clone());
        request.steps.insert(index, Step { item, action });
    } else {
        request.plan.keeps.push(item);
    }
    Ok(())
}

fn config_home(home: &Path) -> anyhow::Result<PathBuf> {
    let home = PathBuf::from(
        home.to_str().ok_or_else(|| anyhow::anyhow!("Claude config home is not Unicode"))?.nfc().collect::<String>(),
    );
    anyhow::ensure!(home.is_absolute(), "Claude auxiliary cache requires an absolute config home");
    match std::fs::canonicalize(&home) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(home),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn uuid(session: &str) -> bool {
    session.len() == 36
        && session
            .bytes()
            .enumerate()
            .all(|(index, byte)| if [8, 13, 18, 23].contains(&index) { byte == b'-' } else { byte.is_ascii_hexdigit() })
}

fn removable(action: &Action) -> anyhow::Result<bool> {
    let Action::ClaudeSessionCache { path, job, sessions } = action else { unreachable!() };
    if crate::claude_work_cache::redirected(path)? {
        return Ok(false);
    }
    let Some(job) = job else { return Ok(true) };
    // Jobs use only eight UUID characters and a new job can overwrite old
    // state without clearing tmp. Nonempty tmp has no exclusive owner proof.
    if !crate::entries(path)?.is_empty() {
        return Ok(false);
    }
    let Some(state) = crate::read::<serde_json::Value>(&job.join("state.json"))? else { return Ok(false) };
    Ok(state
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| sessions.iter().any(|session| session == id)))
}

pub(crate) fn delete(action: &Action) -> anyhow::Result<Vec<PathBuf>> {
    let Action::ClaudeSessionCache { path, job, .. } = action else { unreachable!() };
    if !path.try_exists()? {
        return Ok(Vec::new());
    }
    if !removable(action)? {
        return Ok(vec![path.clone()]);
    }
    if job.is_some() {
        std::fs::remove_dir(path)?;
    } else {
        fastctx_install::uninstall::delete_tree(path)?;
    }
    Ok(Vec::new())
}
