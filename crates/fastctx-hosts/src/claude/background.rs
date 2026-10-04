//! Claude Code background sessions, through the `claude` command itself
//! (`world-projects.md`, 世界请居民进来): starting one, waking one its
//! supervisor stopped, and asking whether one runs.
//!
//! A session is started with `claude --bg` and the World's plugin, settings
//! and instructions; it prints the start of the session's id, and the World's
//! mod reports the whole id when the session starts: that is how the World
//! knows which session it is. Its environment says nothing of it: every background
//! session is spawned by Claude Code's daemon with the daemon's environment,
//! which is that of whichever `claude --bg` started the daemon, the World's
//! or the user's own (Claude Code 2.1.289). One the supervisor stopped is woken with
//! `claude --resume <id> --bg "<words>"` and nothing else: any other flag,
//! or a session still running, starts a copy instead. On Windows an npm
//! install may give only `claude.cmd`; the standard library runs it through
//! the command interpreter with its arguments escaped, and refuses any it
//! cannot escape.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The `claude` command on `path` (the `PATH` the World runs programs with).
pub fn find(path: Option<&OsString>) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd"] } else { &["claude"] };
    let path = path?;
    std::env::split_paths(path)
        .find_map(|directory| names.iter().map(|name| directory.join(name)).find(|candidate| candidate.is_file()))
}

/// How a background session is started.
pub struct Start<'a> {
    pub plugin: &'a Path,
    pub settings: &'a Path,
    pub instructions: &'a Path,
    pub first_words: &'a str,
}

/// `claude --bg` chooses the session's id itself and ignores `--session-id`
/// (it says so); `started` reads the id back from what it prints.
pub fn start_args(start: &Start) -> Vec<OsString> {
    vec![
        "--bg".into(),
        "--plugin-dir".into(),
        start.plugin.into(),
        "--settings".into(),
        start.settings.into(),
        // Nothing asks: the World's tools are allowed by the session's
        // settings and anything else that would ask is refused. Unlike
        // bypassPermissions, this needs no disclaimer the user accepted.
        "--permission-mode".into(),
        "dontAsk".into(),
        "--append-system-prompt-file".into(),
        start.instructions.into(),
        start.first_words.into(),
    ]
}

/// The short id of the session `claude --bg` started, from what it printed:
/// `backgrounded · 1dec4134` and then how to attach to it, stop it and the
/// like (Claude Code 2.1.289). The short id is the first eight hex digits
/// of the session's id, and what `claude stop`, `logs` and `rm` take.
pub fn started(printed: &str) -> Option<String> {
    let line = printed.lines().find(|line| line.contains("backgrounded"))?;
    line.split_whitespace()
        .find(|word| word.len() == 8 && word.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        .map(str::to_string)
}

/// How Claude Code refuses to start a session in a directory not trusted.
pub const UNTRUSTED: &str = "Workspace not trusted";

/// The one way to wake a stopped session as it was.
pub fn wake_args(session: &str, words: &str) -> Vec<OsString> {
    vec!["--resume".into(), session.into(), "--bg".into(), words.into()]
}

/// Runs `claude` with `args` in `cwd` and the environment `env`, and
/// returns what it printed, or why it failed.
pub fn claude(program: &Path, args: &[OsString], cwd: &Path, env: &[(OsString, OsString)]) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).env_clear().envs(env.iter().map(|(name, value)| (name, value)));
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // No console window for a command run from the node.
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|error| format!("running {}: {error}", program.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("claude {} ({}): {}", first(args), output.status, format!("{stdout}{stderr}").trim()))
    }
}

/// Whether the background session `session` has a running process, from
/// `claude agents --json --all`.
pub fn running(agents_json: &str, session: &str) -> Result<bool, String> {
    let agents: serde_json::Value =
        serde_json::from_str(agents_json).map_err(|error| format!("reading `claude agents --json`: {error}"))?;
    let list = agents.as_array().cloned().unwrap_or_default();
    // A session is named by its id, or by the short id `claude --bg` printed
    // until its mod reported the whole of it.
    let named =
        |agent: &serde_json::Value, field: &str| agent.get(field).and_then(serde_json::Value::as_str) == Some(session);
    Ok(list.iter().any(|agent| {
        (named(agent, "sessionId") || named(agent, "id"))
            && agent.get("pid").and_then(serde_json::Value::as_u64).is_some()
    }))
}

fn first(args: &[OsString]) -> String {
    args.first().map(|arg| arg.to_string_lossy().into_owned()).unwrap_or_default()
}
