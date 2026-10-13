//! The record a conversation's agent program keeps of it, outside the
//! World's directory: what it adds to the conversation's size, and what
//! deleting the conversation deletes.
//!
//! Codex keeps a thread's record in files named with the thread's id, under
//! its home's `sessions` and `archived_sessions`, and rows in its databases;
//! the files are counted here, and Codex itself deletes all of it
//! (`thread/delete`). Claude Code keeps a session's record in
//! `projects/<project>/<session>.jsonl` under its home, with what belongs to
//! the session alone in directories named with the session's id: beside that
//! file, and under `file-history`, `session-env`, `uploads` and `dev-mods`.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_types::data::ClaudeKeeping;
use fastctx_types::{ResidentId, layout::WorldDir};

/// The directories Claude Code keeps beside a session's record, named with
/// the session's id.
// Task lists can be shared by sessions; their names do not prove ownership.
const CLAUDE_SESSION_DIRECTORIES: [&str; 4] = ["file-history", "session-env", "uploads", "dev-mods"];

/// The days Claude Code keeps a session untouched when its settings say
/// nothing (2.1.296).
pub(super) const CLAUDE_DEFAULT_DAYS: u32 = 30;

/// The record files of each Codex thread under `home`, by thread id.
pub(super) fn codex_rollouts(home: &Path) -> io::Result<HashMap<String, Vec<PathBuf>>> {
    let mut rollouts: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for root in [home.join("sessions"), home.join("archived_sessions")] {
        collect_rollouts(&root, &mut rollouts)?;
    }
    Ok(rollouts)
}

fn collect_rollouts(directory: &Path, rollouts: &mut HashMap<String, Vec<PathBuf>>) -> io::Result<()> {
    for path in super::space::entries(directory)? {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            collect_rollouts(&path, rollouts)?;
            continue;
        }
        // `rollout-<time>-<thread>.jsonl`, compressed or not; a thread id is
        // a UUID, 36 characters.
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        let Some(stem) = name.strip_prefix("rollout-") else { continue };
        let stem = stem.strip_suffix(".jsonl.zst").or_else(|| stem.strip_suffix(".jsonl"));
        if let Some(stem) = stem
            && stem.len() >= 36
            && let Some(thread) = stem.get(stem.len() - 36..)
        {
            rollouts.entry(thread.to_string()).or_default().push(path);
        }
    }
    Ok(())
}

/// The Claude Code sessions each resident here had, from the directories
/// the World's plugin keeps for them: `(session, its directory)`.
pub(super) fn claude_sessions(world: &WorldDir) -> io::Result<HashMap<ResidentId, Vec<(String, PathBuf)>>> {
    let mut sessions: HashMap<ResidentId, Vec<(String, PathBuf)>> = HashMap::new();
    for directory in super::space::entries(&fastctx_hosts::claude::exchange::sessions(world.path()))? {
        let Some(session) = directory.file_name().and_then(|name| name.to_str()).map(str::to_string) else {
            continue;
        };
        let resident = match std::fs::read(directory.join("resident.json")) {
            Ok(bytes) => serde_json::from_slice::<ResidentId>(&bytes).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        sessions.entry(resident).or_default().push((session, directory));
    }
    Ok(sessions)
}

/// What Claude Code keeps of one session, where the plugin said its record
/// is or, when it did not say, under `home`; and whether the plugin ever
/// said where its record is.
pub(super) fn claude_session_files(
    home: &Path,
    session: &str,
    exchange: Option<&Path>,
) -> io::Result<(Vec<PathBuf>, bool)> {
    // A session's id names files here; one of any other shape names none.
    if session.is_empty() || !session.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-') {
        return Ok((Vec::new(), false));
    }
    let pointed = match exchange {
        Some(exchange) => fastctx_hosts::claude::exchange::transcript(exchange)?,
        None => None,
    };
    let said = pointed.is_some();
    let (home, mut transcripts) = match pointed {
        // `<home>/projects/<project>/<session>.jsonl`
        Some(transcript) => {
            let projects = transcript.parent().and_then(Path::parent);
            if !transcript
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(&format!("{session}.jsonl")))
                || !projects
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("projects"))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Claude reported a transcript path outside its session record layout",
                ));
            }
            let home = transcript
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .map_or_else(|| home.to_path_buf(), Path::to_path_buf);
            (home, vec![transcript])
        }
        None => {
            let mut found = Vec::new();
            for project in super::space::entries(&home.join("projects"))? {
                found.push(project.join(format!("{session}.jsonl")));
            }
            (home.to_path_buf(), found)
        }
    };
    for project in super::space::entries(&home.join("projects"))? {
        let transcript = project.join(format!("{session}.jsonl"));
        if !transcripts.contains(&transcript) {
            transcripts.push(transcript);
        }
    }
    let mut files = Vec::new();
    for transcript in transcripts {
        if transcript.exists() {
            files.push(transcript.clone());
        }
        let beside = transcript.with_extension("");
        if beside.file_name().is_some_and(|name| name == session) && beside.is_dir() {
            files.push(beside);
        }
    }
    for name in CLAUDE_SESSION_DIRECTORIES {
        let directory = home.join(name).join(session);
        if directory.exists() {
            files.push(directory);
        }
    }
    let debug = home.join("debug").join(format!("{session}.txt"));
    if debug.try_exists()? {
        files.push(debug);
    }
    let temporary = std::env::var_os("CLAUDE_CODE_TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    for project in super::space::entries(&temporary.join("claude"))? {
        let images = project.join(session);
        if images.try_exists()? {
            files.push(images);
        }
    }
    Ok((files, said))
}

/// The folder Claude Code keeps under `home`'s `projects` for sessions that
/// stood in `directory`, when there is one. Claude sessions the World started
/// before they stood in their project's directory stood in the conversation's
/// own, so nothing but that conversation is kept there.
pub(super) fn claude_own_project(home: &Path, directory: &Path) -> io::Result<Option<PathBuf>> {
    use fastctx_hosts::claude::projects;
    let folder = home.join("projects").join(projects::slug(&projects::cwd_text(directory)?));
    Ok(folder.is_dir().then_some(folder))
}

pub(super) fn claude_home(fallback: &Path, exchange: Option<&Path>) -> io::Result<PathBuf> {
    let transcript = exchange.map(fastctx_hosts::claude::exchange::transcript).transpose()?.flatten();
    Ok(transcript
        .and_then(|path| {
            let projects = path.parent()?.parent()?;
            (projects.file_name()?.to_str()?.eq_ignore_ascii_case("projects"))
                .then(|| projects.parent().map(Path::to_path_buf))
                .flatten()
        })
        .unwrap_or_else(|| fallback.to_path_buf()))
}

/// How long the Claude Code login at `home` keeps the records of sessions
/// untouched: only `cleanupPeriodDays` of its settings is taken, and nothing
/// else of that file is kept, written anywhere or shown, as it can hold the
/// login's token.
pub(super) fn claude_keeping(home: &Path, default: bool) -> ClaudeKeeping {
    #[derive(serde::Deserialize)]
    struct Settings {
        #[serde(rename = "cleanupPeriodDays", default, deserialize_with = "positive_days")]
        days: Option<u32>,
    }
    fn positive_days<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<Option<u32>, D::Error> {
        let days = <u32 as serde::Deserialize>::deserialize(reader)?;
        if days == 0 {
            return Err(serde::de::Error::custom("cleanupPeriodDays must be positive"));
        }
        Ok(Some(days))
    }
    let shown = (!default).then(|| crate::world::native(home));
    match std::fs::File::open(home.join("settings.json")) {
        Ok(file) => match serde_json::from_reader::<_, Settings>(file) {
            Ok(Settings { days: Some(days) }) => {
                ClaudeKeeping { home: shown, days: Some(days), set: true, unreadable: None }
            }
            Ok(Settings { days: None }) => {
                ClaudeKeeping { home: shown, days: Some(CLAUDE_DEFAULT_DAYS), set: false, unreadable: None }
            }
            // The error says where in the file, never what is in it.
            Err(error) => ClaudeKeeping {
                home: shown,
                days: None,
                set: false,
                unreadable: Some(format!(
                    "its settings.json is invalid at line {}, column {}",
                    error.line(),
                    error.column()
                )),
            },
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            ClaudeKeeping { home: shown, days: Some(CLAUDE_DEFAULT_DAYS), set: false, unreadable: None }
        }
        Err(error) => ClaudeKeeping {
            home: shown,
            days: None,
            set: false,
            unreadable: Some(format!("its settings.json could not be read: {error}")),
        },
    }
}
