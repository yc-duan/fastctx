//! Asking one login who is logged in there, only through the agent's own
//! program (`world-agent-logins.md`, 认出是谁): Codex through the app-server
//! of that login, the one already running or one started for the questions
//! and stopped; Claude Code through `claude auth status --json` with the
//! login's directory named. Nothing is left running.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_hosts::codex::LimitsHow;
use fastctx_types::agents::{AgentModel, Fact, ProgramSource};
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{FoundProgram, Identified, Launcher};
use jiff::Timestamp;

use crate::Node;

/// What asking a login gave.
pub(super) struct Asked {
    pub identified: Identified,
    pub launcher: Option<Launcher>,
    pub models: Vec<AgentModel>,
    /// Codex's limits as read, and when.
    pub limits: Option<(Timestamp, serde_json::Value)>,
}

/// Asks the Codex login in `dir` who is logged in, and with `limits` its
/// usage limits, which are read only when the user asks for them
/// (`world-agent-logins.md`, 额度).
pub(super) async fn codex(node: &Arc<Node>, dir: &Path, limits: bool) -> Asked {
    let at = Timestamp::now();
    if let Some(host) = node.codex.existing(dir)
        && let Some(said) = host.ask_running(limits).await
    {
        // The running app-server's program is the host's launch.
        let launcher = host.launch().ok().flatten().map(|launch| Launcher {
            path: launch.binary,
            version: None,
            desktop_app: launch.source == ProgramSource::DesktopApp,
        });
        return asked_codex(at, said, launcher);
    }
    let launch = match codex_launch(node, dir) {
        Ok(Some(launch)) => launch,
        Ok(None) => {
            return failed(at, "neither the ChatGPT desktop app nor a codex command was found to ask it".into());
        }
        Err(error) => return failed(at, error),
    };
    let cwd = match node.codex.asking_dir(dir) {
        Ok(cwd) => cwd,
        Err(failure) => return failed(at, failure.message),
    };
    let launcher = Launcher {
        path: launch.binary.clone(),
        version: None,
        desktop_app: launch.source == ProgramSource::DesktopApp,
    };
    match fastctx_hosts::codex::identify::ask_once(
        &launch,
        dir,
        &cwd,
        node.codex.environment(),
        node.codex.log(),
        limits,
    )
    .await
    {
        Ok(said) => asked_codex(at, said, Some(launcher)),
        Err(error) => Asked {
            identified: Identified { at, who: Fact::failed(error) },
            launcher: Some(launcher),
            models: Vec::new(),
            limits: None,
        },
    }
}

/// The Codex the login in `dir` starts now, as the node starts it: what the
/// desktop app records in the configuration, read again each time, since the
/// app moves its program when it upgrades (`fastctx_hosts::codex::launch`).
pub(super) fn codex_launch(node: &Node, dir: &Path) -> Result<Option<fastctx_hosts::codex::launch::Launch>, String> {
    let path = node
        .codex
        .environment()
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
        .map(|(_, value)| value.clone());
    let default = node.codex.default().config_home().to_path_buf();
    let program_home = (!crate::homes::same_dir(&default, dir)).then_some(default);
    fastctx_hosts::codex::launch::discover_for(dir, program_home.as_deref(), path.as_ref())
}

/// The program a Codex login was last asked with, as it stands now: the
/// desktop app's moves with each upgrade, so the one recorded is looked up
/// again; the recorded version holds only while the program is the same.
pub(super) fn current_launcher(node: &Node, dir: &Path, recorded: Launcher) -> Launcher {
    if !recorded.desktop_app {
        return recorded;
    }
    match codex_launch(node, dir) {
        Ok(Some(launch)) if launch.binary != recorded.path => {
            Launcher { path: launch.binary, version: None, desktop_app: launch.source == ProgramSource::DesktopApp }
        }
        _ => recorded,
    }
}

fn asked_codex(at: Timestamp, said: fastctx_hosts::codex::identify::Said, launcher: Option<Launcher>) -> Asked {
    let launcher = launcher.map(|launcher| Launcher { version: said.version.clone(), ..launcher });
    let limits = match said.limits {
        Some(Ok(read)) => Some(read),
        _ => None,
    };
    Asked {
        identified: Identified { at, who: said.who },
        launcher,
        models: match said.models {
            Fact::Known { value } => value,
            _ => Vec::new(),
        },
        limits,
    }
}

fn failed(at: Timestamp, reason: String) -> Asked {
    Asked { identified: Identified { at, who: Fact::failed(reason) }, launcher: None, models: Vec::new(), limits: None }
}

/// The `claude` command the World runs: the first on the node's search
/// path, as for every Claude Code command it runs (`claude.rs`).
pub(super) fn claude_program() -> Option<PathBuf> {
    fastctx_hosts::claude::background::find(std::env::var_os("PATH").as_ref())
}

/// Asks the Claude Code login in `dir`; `programs` are the Claude programs
/// found here, whose records say the command's version.
pub(super) fn claude(dir: &Path, default: bool, programs: &[FoundProgram]) -> Result<Asked, Failure> {
    let at = Timestamp::now();
    let Some(program) = claude_program() else {
        return Ok(Asked {
            identified: Identified { at, who: Fact::unavailable("no claude command is on the node's search path") },
            launcher: None,
            models: Vec::new(),
            limits: None,
        });
    };
    let mut env: Vec<(OsString, OsString)> = crate::claude::environment()?;
    if !default {
        env.retain(|(name, _)| !name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"));
        env.push(("CLAUDE_CONFIG_DIR".into(), dir.as_os_str().to_owned()));
    }
    let cwd =
        fastctx_platform::user_home().map_err(|error| Failure::new(format!("finding the home directory: {error}")))?;
    let who = fastctx_hosts::agents::claude_identity(&program, &cwd, &env);
    let version = programs
        .iter()
        .find(|found| found.path.as_ref().is_some_and(|path| fastctx_hosts::inventory::same_file(path, &program)))
        .and_then(|found| found.version.clone());
    Ok(Asked {
        identified: Identified { at, who },
        launcher: Some(Launcher { path: program, version, desktop_app: false }),
        models: Vec::new(),
        limits: None,
    })
}

/// The limits a Codex read gave, as the node records them.
pub(super) fn limits_read(dir: &Path, at: Timestamp, answer: serde_json::Value) -> fastctx_hosts::codex::LimitsReport {
    fastctx_hosts::codex::LimitsReport { home: dir.to_path_buf(), at, how: LimitsHow::Read(answer) }
}
