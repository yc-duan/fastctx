//! What the World's Claude sessions leave behind: exchange leftovers retire
//! when their writer and request helper have ended, and what Claude Code
//! keeps for deleted conversations' directories goes.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_hosts::claude::{control, exchange, projects, trust};
use fastctx_types::layout::WorldDir;

use crate::Node;

/// Collects what each session left; one that cannot be collected is told
/// and the others still are.
pub(crate) fn collect(world: &WorldDir) -> io::Result<()> {
    let mut sessions = crate::retention::entries(&exchange::sessions(world.path()))?;
    for kept in world.kept()? {
        sessions.extend(crate::retention::entries(&kept.directory.path().join("claude"))?);
    }
    let failures: Vec<String> = sessions
        .iter()
        .filter_map(|session| collect_session(session).err().map(|error| format!("{}: {error}", session.display())))
        .collect();
    if failures.is_empty() { Ok(()) } else { Err(io::Error::other(failures.join("; "))) }
}

fn collect_session(session: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(session)?;
    if fastctx_platform::owned_directory::is_link(&metadata) || !metadata.is_dir() {
        return Ok(());
    }
    let alive = exchange::registration(session)?
        .and_then(|registration| registration.process)
        .map(|process| fastctx_platform::is_alive(&process))
        .transpose()?
        .unwrap_or(false);
    if !alive {
        for directory in [session.to_path_buf(), session.join("events"), session.join("control")] {
            parts(&directory, &session.join("damaged"))?;
        }
        let retired = session.parent().and_then(Path::parent).is_some_and(|home| home.join("books-retired").exists());
        if retired {
            for name in ["applied", "receipts", "attempts", "closed", "calls", "inbox-parts"] {
                fastctx_platform::owned_directory::delete(&session.join(name))?;
            }
        }
    }
    let directory = session.join("control");
    if let Some(host) = control::host(&directory)?
        && !fastctx_platform::is_alive(&host.process)?
    {
        for side in ["inbox", "replies"] {
            fastctx_platform::owned_directory::delete(&directory.join(side))?;
        }
    }
    for path in control::requests(&directory)? {
        if let Some(waiting) = control::read::<control::Waiting>(&path.join("request.json"))?
            && !fastctx_platform::is_alive(&waiting.process)?
            && path.join("resolved.json").try_exists()?
        {
            fastctx_platform::owned_directory::delete(&path)?;
        }
    }
    readies(&directory)
}

/// How long what a control left stays once nothing may take it again, for
/// a taking that raced its collection.
const SETTLED: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Whether the file at `path` was last written longer ago than `SETTLED`.
fn settled(entry: &std::fs::DirEntry, now: std::time::SystemTime) -> io::Result<bool> {
    let modified = entry.metadata()?.modified()?;
    Ok(now.duration_since(modified).is_ok_and(|age| age >= SETTLED))
}

/// Each activation of the session's mod says it is ready once; only the
/// activation it runs as now is read.
fn readies(control: &Path) -> io::Result<()> {
    // Without its present activation known, none is taken for a past one.
    let Some(current) = control::host(control)?.map(|host| host.instance) else { return Ok(()) };
    let entries = match std::fs::read_dir(control) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let now = std::time::SystemTime::now();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(instance) = name
            .to_str()
            .and_then(|name| name.strip_prefix("ready-"))
            .and_then(|rest| rest.strip_suffix(".json").or_else(|| rest.strip_suffix(".done")))
        else {
            continue;
        };
        if instance == current || !settled(&entry, now)? {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// Takes back the trust the World gave in Claude Code to the directories of
/// conversations deleted since, and the folders Claude Code kept for them.
/// Sessions the World started stood in their conversation's own directory
/// before they stood in their project's; a conversation deleted then took
/// its sessions' records, but left these.
pub(crate) fn untrust_departed(node: &Node) -> io::Result<()> {
    let conversations = node.world.conversations();
    if !conversations.is_dir() {
        return Ok(());
    }
    let key = trust::uninstall_key(&conversations)?;
    let env = crate::claude::environment().map_err(|failure| io::Error::other(failure.message))?;
    let default = crate::homes::claude_default(&env, &fastctx_platform::user_home()?);
    let default_config =
        env.iter().find(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR")).map(|(_, value)| value.clone());
    let mut homes: Vec<Option<PathBuf>> = vec![None];
    for record in node.residents.everyone() {
        if record.about.world_started && record.about.host.agent.as_str() == "claude" {
            homes.push(record.about.home);
        }
    }
    homes.sort();
    homes.dedup();
    for home in homes {
        let config = home.clone().map(PathBuf::into_os_string).or_else(|| default_config.clone());
        for entry in trust::world_entries(config.as_deref(), &key)? {
            let directory = PathBuf::from(&entry);
            if directory.try_exists()? {
                continue;
            }
            trust::remove_world_trust(config.as_deref(), &entry)?;
            let claude = home.as_deref().unwrap_or(&default);
            let folder = claude.join("projects").join(projects::slug(&projects::cwd_text(&directory)?));
            if folder.is_dir() {
                fastctx_platform::owned_directory::delete(&folder)?;
            }
            node.log.line(format_args!("took back Claude Code's trust of {entry}, a deleted conversation's directory"));
        }
    }
    Ok(())
}

fn parts(directory: &Path, damaged: &Path) -> io::Result<()> {
    for path in crate::retention::entries(directory)? {
        if path.extension().is_none_or(|extension| extension != "parts") {
            continue;
        }
        let manifest = path.with_extension("json");
        let count = match std::fs::read(&manifest) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
                .map_err(io::Error::other)?
                .get("parts")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if let Some(name) = manifest.file_name()
                    && damaged.join(name).try_exists()?
                {
                    fastctx_platform::owned_directory::move_to(
                        &path,
                        &damaged.join(path.file_name().expect("parts name")),
                    )?;
                    continue;
                }
                0
            }
            Err(error) => return Err(error),
        };
        if count == 0 {
            fastctx_platform::owned_directory::delete(&path)?;
        } else {
            for piece in crate::retention::entries(&path)? {
                if piece
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.parse::<u64>().ok())
                    .is_some_and(|index| index >= count)
                {
                    fastctx_platform::owned_directory::delete(&piece)?;
                }
            }
        }
    }
    Ok(())
}
