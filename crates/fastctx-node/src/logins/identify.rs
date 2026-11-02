//! Who is logged in to a login, as the files in its directory say
//! (`world-agent-logins.md`, 认出是谁): nothing is started and nothing goes
//! over the network, and the program the login starts is found on disk. On
//! the workbench a Codex login's program is asked as well, through an
//! official process started for the questions and stopped, which models it
//! has and which version it is, for the workbench's conversations; its
//! limits only when the user asks for them (`limits::read`).

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

use fastctx_hosts::codex::LimitsHow;
use fastctx_types::access::Acting;
use fastctx_types::agents::{AgentModel, Fact, ProgramSource};
use fastctx_types::layers::Layer;
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

/// What the record keeps of `agent`'s login `dir`, the agent's default one
/// or not: who its files say is logged in, and the program it starts with
/// its version as `programs`, the agent's programs found here, record it;
/// on the workbench, a Codex login's models and version as its program says
/// them. `env` is the user's environment, which places Claude Code's files.
pub(super) async fn asked(
    node: &Arc<Node>,
    agent: &'static str,
    dir: &Path,
    default: bool,
    programs: Vec<FoundProgram>,
    env: Vec<(OsString, OsString)>,
) -> Asked {
    let (finding, path) = (node.clone(), dir.to_path_buf());
    let read =
        fastctx_platform::task::blocking(move || on_disk(&finding, agent, &path, default, &programs, &env)).await;
    if agent != "codex" || crate::layers::of(node, &Acting::Here) != Layer::Workbench {
        return read;
    }
    let said = codex(node, dir, false).await;
    Asked {
        // The files say who; the program only where they cannot, a sign-in
        // kept in the system's keyring.
        identified: if matches!(read.identified.who, Fact::Known { .. }) { read.identified } else { said.identified },
        launcher: said.launcher.or(read.launcher),
        models: said.models,
        limits: None,
    }
}

fn on_disk(
    node: &Node,
    agent: &str,
    dir: &Path,
    default: bool,
    programs: &[FoundProgram],
    env: &[(OsString, OsString)],
) -> Asked {
    let at = Timestamp::now();
    let (who, program) = if agent == "codex" {
        let program = match node.codex.launch(dir) {
            Ok(Some(launch)) => Some((launch.binary, launch.source == ProgramSource::DesktopApp)),
            Ok(None) | Err(_) => None,
        };
        (fastctx_hosts::codex::login_files::identity(dir), program)
    } else {
        let named =
            |name: &str| env.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_os_str());
        // The World starts a login's sessions with its directory named,
        // the default one as the user's environment has it.
        let configured: Option<&OsStr> = if default { named("CLAUDE_CONFIG_DIR") } else { Some(dir.as_os_str()) };
        let who = fastctx_hosts::claude::login_files::identity(configured, named("CLAUDE_SECURESTORAGE_CONFIG_DIR"));
        (who, crate::claude::program().map(|path| (path, false)))
    };
    let launcher = program.map(|(path, desktop_app)| Launcher {
        version: programs
            .iter()
            .find(|found| found.path.as_ref().is_some_and(|known| fastctx_hosts::inventory::same_file(known, &path)))
            .and_then(|found| found.version.clone()),
        path,
        desktop_app,
    });
    Asked { identified: Identified { at, who }, launcher, models: Vec::new(), limits: None }
}

/// Asks the Codex login in `dir` through its program who is logged in, its
/// models and its version, and with `limits` its usage limits: on the
/// workbench only (`asked`, `limits::read`).
pub(super) async fn codex(node: &Arc<Node>, dir: &Path, limits: bool) -> Asked {
    let at = Timestamp::now();
    // A running app-server can retain the previous directory login. Identification
    // uses a short-lived official process so a rescan cannot restore that owner.
    // The program and the directory to ask it in are found on disk.
    let found = {
        let (finding, dir) = (node.clone(), dir.to_path_buf());
        fastctx_platform::task::blocking(move || (finding.codex.launch(&dir), finding.codex.asking_dir(&dir))).await
    };
    let (launch, asking) = found;
    let launch = match launch {
        Ok(Some(launch)) => launch,
        Ok(None) => {
            return failed(at, "neither the ChatGPT desktop app nor a codex command was found to ask it".into());
        }
        Err(error) => return failed(at, error),
    };
    let cwd = match asking {
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
        true,
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

/// The limits a Codex read gave, as the node records them.
pub(super) fn limits_read(
    dir: &Path,
    at: Timestamp,
    answer: serde_json::Value,
    who: &Fact<fastctx_types::logins::Identity>,
) -> fastctx_hosts::codex::LimitsReport {
    let owner = match who {
        Fact::Known { value } => Some((at, value.clone())),
        _ => None,
    };
    fastctx_hosts::codex::LimitsReport { home: dir.to_path_buf(), at, how: LimitsHow::Read(answer), owner }
}
