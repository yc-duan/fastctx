//! Claude Code 2.1.296 keeps diagnostics and task output outside its login
//! directory, keyed by the working directory. Session history is excluded.

use crate::claude_cache_paths::Roots;
use crate::{Action, Request, Step};
use fastctx_hosts::claude::projects::{cwd_text, slug};
use fastctx_types::uninstall::Item;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};

pub(crate) fn include_steps(request: &mut Request) -> anyhow::Result<()> {
    let roots = Roots::here(&request.installation)?;
    include_at(request, &roots)
}

pub(crate) fn include_at(request: &mut Request, roots: &Roots) -> anyhow::Result<()> {
    let world = request.installation.root.join("world");
    anyhow::ensure!(
        !redirected(&world)? && !redirected(&world.join("outside"))? && !redirected(&world.join("towns"))?,
        "a World working directory is redirected; its Claude cache cannot be claimed by cwd"
    );
    let mut directories = vec![world.join("outside")];
    for town in crate::entries(&world.join("towns"))? {
        if town.file_type()?.is_dir() {
            anyhow::ensure!(
                !redirected(&town.path())?,
                "a World town is redirected; its Claude cache cannot be claimed by cwd"
            );
            directories.push(town.path());
        }
    }
    let mut working_directories =
        directories.iter().map(|directory| cwd_text(directory)).collect::<std::io::Result<Vec<_>>>()?;
    let world_cwd = PathBuf::from(cwd_text(&world)?);
    // A previous removal can have erased towns already. The name is only a
    // discovery filter; the log's actual cwd supplies the ownership evidence.
    let prefix = world_cwd
        .to_str()
        .expect("the normalized cwd is Unicode")
        .encode_utf16()
        .take(200)
        .map(|unit| if unit <= 127 && (unit as u8).is_ascii_alphanumeric() { unit as u8 as char } else { '-' })
        .collect::<String>();
    for entry in crate::entries(&roots.cache)? {
        if !entry.file_type()?.is_dir() || !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        for file in log_files(&entry.path())?.0 {
            for line in BufReader::new(std::fs::File::open(file)?).split(b'\n') {
                let line = line?;
                if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&line)
                    && let Some(cwd) = row.get("cwd").and_then(serde_json::Value::as_str)
                    && own_cwd(Path::new(cwd), &world_cwd)
                {
                    working_directories.push(cwd.to_owned());
                }
            }
        }
    }
    let mut sessions = BTreeSet::new();
    let mut residents = BTreeSet::new();
    let layout = fastctx_types::layout::WorldDir::at(&world);
    for file in crate::inventory::abouts(&layout)? {
        if let Some(about) = crate::read::<fastctx_types::residents::About>(&file)?
            && about.world_started
            && about.host.agent.as_str() == "claude"
            && let Some(session) = about.session
        {
            anyhow::ensure!(single_component(&session), "Claude session identity is not a directory component");
            if session.len() != 8 || !session.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                sessions.insert(session.clone());
            }
            residents.insert(about.id);
            if let Some(registration) =
                crate::read::<serde_json::Value>(&world.join("claude").join(&session).join("session.json"))?
                && let Some(cwd) = registration.get("cwd").and_then(serde_json::Value::as_str)
            {
                let cwd = PathBuf::from(cwd);
                if own_cwd(&cwd, &world_cwd) {
                    working_directories.push(cwd.to_str().expect("a JSON cwd is Unicode").to_owned());
                }
            }
        }
    }
    for registration in crate::entries(&world.join("claude"))? {
        let Some(session) =
            registration.file_name().to_str().filter(|name| crate::claude_session_cache::uuid(name)).map(str::to_owned)
        else {
            continue;
        };
        if let Some(resident) = crate::read::<fastctx_types::ResidentId>(&registration.path().join("resident.json"))?
            && residents.contains(&resident)
        {
            sessions.insert(session);
            if let Some(metadata) = crate::read::<serde_json::Value>(&registration.path().join("session.json"))?
                && let Some(cwd) = metadata.get("cwd").and_then(serde_json::Value::as_str)
                && own_cwd(Path::new(cwd), &world_cwd)
            {
                working_directories.push(cwd.to_owned());
            }
        }
    }
    working_directories.sort();
    working_directories.dedup();
    let mut keys = BTreeMap::<String, Vec<String>>::new();
    for working_directory in working_directories {
        keys.entry(slug(&working_directory)).or_default().push(working_directory);
    }
    for (key, working_directories) in keys {
        let diagnostics = roots.cache.join(&key);
        let mut known_sessions = sessions.clone();
        extend_log_sessions(&diagnostics, &working_directories, &mut known_sessions)?;
        for (path, temporary) in [(roots.temporary.join(&key), true), (diagnostics.clone(), false)] {
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            }
            let location = path.display().to_string();
            if request.steps.iter().any(
                |step| matches!(&step.action, Action::ClaudeWorkCache { directory: existing, .. } if existing == &path),
            ) || request
                .plan
                .keeps
                .iter()
                .any(|item| item.kind == "claude-work-cache" && item.location.as_deref() == Some(&location))
            {
                continue;
            }
            let item = Item {
                id: format!("claude-work-cache-{location}"),
                kind: "claude-work-cache".into(),
                what: "World-owned Claude Code cache, logs and temporary files for this working directory".into(),
                location: Some(location),
                admin: false,
            };
            if request.plan.everything {
                let action = Action::ClaudeWorkCache {
                    directory: path,
                    diagnostics: diagnostics.clone(),
                    working_directories: working_directories.clone(),
                    sessions: known_sessions.iter().cloned().collect(),
                    temporary,
                };
                let (_, left) = classified(&action)?;
                for path in left {
                    let location = path.display().to_string();
                    if !request.plan.keeps.iter().any(|item| item.location.as_deref() == Some(&location)) {
                        request.plan.keeps.push(Item {
                            id: format!("claude-cache-unrecognized-{location}"),
                            kind: "claude-shared-cache".into(),
                            what: "Claude cache entry without exclusive World ownership".into(),
                            location: Some(location),
                            admin: false,
                        });
                    }
                }
                // Capture these paths before the World directories are erased,
                // including when resuming a checkpoint written by an older build.
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
        }
    }
    Ok(())
}

fn own_cwd(cwd: &Path, world: &Path) -> bool {
    cwd == world.join("outside") || cwd.parent() == Some(world.join("towns").as_path())
}

pub(crate) fn remember_sessions(steps: &mut [Step]) -> anyhow::Result<()> {
    for step in steps {
        if let Action::ClaudeWorkCache { diagnostics, working_directories, sessions, .. } = &mut step.action {
            let mut known = sessions.iter().cloned().collect::<BTreeSet<_>>();
            extend_log_sessions(diagnostics, working_directories, &mut known)?;
            *sessions = known.into_iter().collect();
        }
    }
    Ok(())
}

fn extend_log_sessions(
    directory: &Path,
    working_directories: &[String],
    sessions: &mut BTreeSet<String>,
) -> anyhow::Result<()> {
    for path in log_files(directory)?.0 {
        owned_log(&path, working_directories, sessions)?;
    }
    Ok(())
}

fn single_component(text: &str) -> bool {
    !text.is_empty() && text != "." && text != ".." && !text.contains(['/', '\\'])
}

fn owned_log(path: &Path, working_directories: &[String], sessions: &mut BTreeSet<String>) -> anyhow::Result<bool> {
    let file = std::fs::File::open(path)?;
    let mut found = false;
    let mut exclusive = true;
    let mut identities = BTreeSet::new();
    for line in BufReader::new(file).split(b'\n') {
        let line = line?;
        let Ok(row) = serde_json::from_slice::<serde_json::Value>(&line) else {
            exclusive = false;
            continue;
        };
        let Some(cwd) = row.get("cwd").and_then(serde_json::Value::as_str) else {
            exclusive = false;
            continue;
        };
        let Some(session) = row.get("sessionId").and_then(serde_json::Value::as_str).filter(|id| single_component(id))
        else {
            exclusive = false;
            continue;
        };
        if !working_directories.iter().any(|directory| directory == cwd) && !sessions.contains(session) {
            exclusive = false;
            continue;
        }
        identities.insert(session.to_owned());
        found = true;
    }
    sessions.extend(identities);
    Ok(found && exclusive)
}

fn log_files(directory: &Path) -> anyhow::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut logs = Vec::new();
    let mut other = Vec::new();
    if redirected(directory)? {
        return Ok((logs, vec![directory.to_path_buf()]));
    }
    for server in crate::entries(directory)? {
        if !server.file_name().to_string_lossy().starts_with("mcp-logs-")
            || !server.file_type()?.is_dir()
            || redirected(&server.path())?
        {
            other.push(server.path());
            continue;
        }
        let entries = crate::entries(&server.path())?;
        if entries.is_empty() {
            other.push(server.path());
        }
        for entry in entries {
            if entry.file_type()?.is_file()
                && entry.path().extension() == Some(OsStr::new("jsonl"))
                && !redirected(&entry.path())?
            {
                logs.push(entry.path());
            } else {
                other.push(entry.path());
            }
        }
    }
    Ok((logs, other))
}

fn classified(action: &Action) -> anyhow::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let Action::ClaudeWorkCache { directory, diagnostics, working_directories, sessions, temporary } = action else {
        unreachable!()
    };
    if redirected(directory)? {
        return Ok((Vec::new(), vec![directory.clone()]));
    }
    let mut sessions = sessions.iter().cloned().collect::<BTreeSet<_>>();
    let (logs, mut left) = log_files(diagnostics)?;
    let mut owned = Vec::new();
    for file in logs {
        if owned_log(&file, working_directories, &mut sessions)? {
            owned.push(file);
        } else {
            left.push(file);
        }
    }
    if *temporary {
        owned.clear();
        left.clear();
        for entry in crate::entries(directory)? {
            if entry.file_type()?.is_dir()
                && entry.file_name().to_str().is_some_and(|name| sessions.contains(name))
                && !redirected(&entry.path())?
            {
                owned.push(entry.path());
            } else {
                left.push(entry.path());
            }
        }
    }
    if owned.is_empty() && left.is_empty() && directory.try_exists()? {
        left.push(directory.clone());
    }
    Ok((owned, left))
}

pub(crate) fn delete(action: &Action) -> anyhow::Result<Vec<PathBuf>> {
    let Action::ClaudeWorkCache { directory, temporary, .. } = action else { unreachable!() };
    if redirected(directory)? {
        return Ok(vec![directory.clone()]);
    }
    let (owned, mut left) = classified(action)?;
    let removed_owned = !owned.is_empty();
    if *temporary {
        for path in owned {
            fastctx_install::uninstall::delete_tree(&path)?;
        }
    } else {
        let parents = owned.iter().filter_map(|file| file.parent().map(Path::to_path_buf)).collect::<BTreeSet<_>>();
        for file in owned {
            fastctx_install::uninstall::delete_file(&file)?;
        }
        for server in crate::entries(directory)? {
            if parents.contains(&server.path()) && crate::entries(&server.path())?.is_empty() {
                std::fs::remove_dir(server.path())?;
            }
        }
    }
    if removed_owned && directory.try_exists()? && crate::entries(directory)?.is_empty() {
        std::fs::remove_dir(directory)?;
    } else if directory.try_exists()? && left.is_empty() {
        left.push(directory.clone());
    }
    Ok(left)
}

pub(crate) fn redirected(path: &Path) -> std::io::Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        // Reparse points include directory junctions, not just symlinks.
        Ok(metadata.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    Ok(metadata.is_symlink())
}
