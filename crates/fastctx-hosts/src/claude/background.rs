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
//! or a session still running, starts a copy instead, with none of the
//! World's settings. So does a session Claude Code cannot tell is not open
//! elsewhere, as when its records of open sessions name a process of
//! another machine, which it takes for running (2.1.303): such a session
//! the World continues in a new one of its own (`continue_args`). On Windows an npm
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
    pub mcp: &'a Path,
    pub instructions: &'a Path,
    pub first_words: &'a str,
    pub model: Option<&'a str>,
    pub reasoning_effort: Option<&'a str>,
    /// The login assists another account: only the explicitly supplied World
    /// plugin, MCP configuration and session settings are loaded.
    pub assisting: bool,
}

/// `claude --bg` chooses the session's id itself and ignores `--session-id`
/// (it says so); `started` reads the id back from what it prints.
pub fn start_args(start: &Start) -> Vec<OsString> {
    let mut args = settings_args(start);
    args.extend(["--".into(), start.first_words.into()]);
    args
}

/// Forks the entire saved conversation into an idle background session.
/// No synthetic prompt is attributed to the person who pressed Fork.
pub fn fork_args(session: &str, start: &Start) -> Vec<OsString> {
    let mut args = settings_args(start);
    args.extend(["--resume".into(), session.into(), "--fork-session".into()]);
    args
}

/// Continues the saved conversation `session` in a new background session
/// with the World's settings, given its first words when there are any:
/// for a session Claude Code will not wake as itself, which it would only
/// ever copy without them.
pub fn continue_args(session: &str, start: &Start) -> Vec<OsString> {
    let mut args = fork_args(session, start);
    if !start.first_words.is_empty() {
        args.extend(["--".into(), start.first_words.into()]);
    }
    args
}

/// Why Claude Code started a copy instead of waking the session it was
/// asked for, as it tells it on its error stream (`Printed::told`):
/// `note: session 1dec4134 is open in another Claude Code process, so this
/// started a copy as 9b0e2f11. …` (2.1.303).
pub fn copy_note(told: &str) -> Option<&str> {
    told.lines().map(str::trim).find(|line| line.starts_with("note:") && line.contains("started a copy"))
}

fn settings_args(start: &Start) -> Vec<OsString> {
    let mut args = vec![
        "--bg".into(),
        "--plugin-dir".into(),
        start.plugin.into(),
        "--settings".into(),
        start.settings.into(),
        "--mcp-config".into(),
        start.mcp.into(),
        // Nothing waits for approval and nothing is refused for want of it:
        // the session can do whatever Claude Code can where the user works.
        // Claude Code starts it only once the user has accepted this mode
        // in an interactive session on this machine.
        "--permission-mode".into(),
        "bypassPermissions".into(),
        "--append-system-prompt-file".into(),
        start.instructions.into(),
    ];
    if start.assisting {
        args.extend(["--no-chrome".into(), "--strict-mcp-config".into(), "--setting-sources=".into()]);
    }
    if let Some(model) = start.model {
        args.extend(["--model".into(), model.into()]);
    }
    if let Some(effort) = start.reasoning_effort {
        args.extend(["--effort".into(), effort.into()]);
    }
    args
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

/// The one way to wake a stopped session as it was. The words follow `--`,
/// as when a session starts: words that begin with `-` are not options.
pub fn wake_args(session: &str, words: &str) -> Vec<OsString> {
    vec!["--resume".into(), session.into(), "--bg".into(), "--".into(), words.into()]
}

/// Runs `claude` with `args` in `cwd` and the environment `env`, and
/// returns what it printed, or why it failed: past its deadline (`within`)
/// the command is ended and that is said (`specs/world-concurrency.md`).
pub fn claude(program: &Path, args: &[OsString], cwd: &Path, env: &[(OsString, OsString)]) -> Result<String, String> {
    invoke(program, args, cwd, env, within(args)).map_err(|failure| failure.message)
}

/// `claude`, keeping what the command told on its error stream.
pub fn claude_telling(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> Result<Printed, String> {
    invoke_telling(program, args, cwd, env, within(args)).map_err(|failure| failure.message)
}

/// A CLI error is retryable only when the process was never started.
/// A nonzero exit can follow an accepted prompt, including a trust refusal.
#[derive(Debug)]
pub struct CommandFailure {
    pub message: String,
    pub started: bool,
}

/// Runs a host operation with an explicit deadline and preserves whether
/// it crossed the process boundary. Callers reconcile started failures.
pub fn invoke(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    env: &[(OsString, OsString)],
    deadline: std::time::Duration,
) -> Result<String, CommandFailure> {
    invoke_telling(program, args, cwd, env, deadline).map(|printed| printed.out)
}

/// What a command that succeeded printed: its output, and what it told on
/// its error stream, where Claude Code says why it started a copy
/// (`copy_note`).
pub struct Printed {
    pub out: String,
    pub told: String,
}

/// `invoke`, keeping what the command told on its error stream.
pub fn invoke_telling(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    env: &[(OsString, OsString)],
    deadline: std::time::Duration,
) -> Result<Printed, CommandFailure> {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).env_clear().envs(env.iter().map(|(name, value)| (name, value)));
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // No console window for a command run from the node.
        command.creation_flags(0x0800_0000);
    }
    let what = format!("`claude {}`", first(args));
    let output = fastctx_platform::bounded::output_tracked(command, &what, deadline).map_err(|failure| {
        let message = if fastctx_platform::bounded::ran_late(&failure.error) {
            format!("Claude Code did not answer {what} within {} s", deadline.as_secs())
        } else {
            format!("running {}: {}", program.display(), failure.error)
        };
        CommandFailure { message, started: failure.started }
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(Printed { out: stdout, told: String::from_utf8_lossy(&output.stderr).into_owned() })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(CommandFailure {
            message: format!("claude {} ({}): {}", first(args), output.status, format!("{stdout}{stderr}").trim()),
            started: true,
        })
    }
}

/// How long a `claude` command may take before it counts as not answering:
/// starting, waking or forking a session waits for Claude Code's daemon to
/// spawn it; stopping one waits for it to end; listing and reading are quick.
fn within(args: &[OsString]) -> std::time::Duration {
    let starts = args.iter().any(|arg| arg == "--bg" || arg == "--resume" || arg == "--fork-session");
    let seconds = match args.first().and_then(|arg| arg.to_str()) {
        _ if starts => 120,
        Some("stop" | "rm" | "plugin") => 60,
        _ => 30,
    };
    std::time::Duration::from_secs(seconds)
}

/// Whether the background session `session` has a running process, from
/// `claude agents --json --all`.
/// Whether the session rests: nothing `claude agents --json --all` lists
/// for it has a process, and Claude Code is not starting or working its
/// background session (2.1.303 names a job's state `starting`, `working`,
/// `blocked`, `done`, `failed` or `stopped`). A process is listed from the
/// records of open sessions, of this machine only.
pub fn resting(agents_json: &str, session: &str) -> Result<bool, String> {
    let list: Vec<serde_json::Value> =
        serde_json::from_str(agents_json).map_err(|error| format!("reading `claude agents --json`: {error}"))?;
    let named =
        |agent: &serde_json::Value, field: &str| agent.get(field).and_then(serde_json::Value::as_str) == Some(session);
    Ok(list.iter().filter(|agent| named(agent, "sessionId") || named(agent, "id")).all(|agent| {
        agent.get("pid").and_then(serde_json::Value::as_u64).is_none()
            && matches!(
                agent.get("state").and_then(serde_json::Value::as_str),
                None | Some("blocked" | "done" | "failed" | "stopped")
            )
    }))
}

pub fn running(agents_json: &str, session: &str) -> Result<bool, String> {
    let list: Vec<serde_json::Value> =
        serde_json::from_str(agents_json).map_err(|error| format!("reading `claude agents --json`: {error}"))?;
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
