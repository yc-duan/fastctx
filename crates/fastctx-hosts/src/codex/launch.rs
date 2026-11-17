//! Which Codex to start, and how (`world-colleagues.md`, 世界怎样拉起 GPT
//! 同事).
//!
//! The ChatGPT desktop app records its own `codex.exe` in the user's Codex
//! configuration, as the `CODEX_CLI_PATH` of its Computer Use server
//! (`[mcp_servers.node_repl.env]`). That binary, started with the flags the
//! app uses, gives every thread the app's Computer Use, plugins and
//! connectors. The app moves the binary when it upgrades and renames the
//! Computer Use pipe when it restarts, rewriting the configuration each
//! time, so the configuration is read again before every start. Until an
//! upgraded app first runs, the paths it wrote name the removed version;
//! they follow the app to its installed version (`super::app_package`).
//! Without the app, a standalone `codex` on the search path speaks the same
//! protocol.

use fastctx_types::agents::ProgramSource;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A Codex to start, with the desktop integration it will inherit.
#[derive(Clone, Debug)]
pub struct Launch {
    pub binary: PathBuf,
    pub source: ProgramSource,
    pub args: Vec<String>,
    computer_use: Option<toml::Value>,
    pub(super) assisted_config: Option<toml::Table>,
}

impl Launch {
    /// Only changes to the selected executable, launch arguments or desktop
    /// tool connection require replacing the process. Codex itself writes
    /// project trust into config.toml while starting threads (0.160).
    pub fn same_runtime(&self, other: &Self) -> bool {
        self.binary == other.binary
            && self.args == other.args
            && self.computer_use == other.computer_use
            && self.assisted_config == other.assisted_config
    }
}

/// The user's Codex directory: `CODEX_HOME` when it is set, as Codex reads
/// it, and `.codex` in the home directory otherwise.
pub fn codex_home(home: &Path, env: &[(OsString, OsString)]) -> PathBuf {
    match env.iter().find(|(name, _)| name == "CODEX_HOME").map(|(_, value)| value) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(".codex"),
    }
}

pub(crate) fn read_config(codex_home: &Path) -> Result<toml::Table, String> {
    let path = codex_home.join("config.toml");
    let text = match fastctx_platform::disk::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(toml::Table::new()),
        Err(error) => return Err(format!("reading {}: {error}", path.display())),
    };
    text.parse().map_err(|error: toml::de::Error| format!("reading {}: {}", path.display(), error.message()))
}

/// The Codex to start now, looking for the program on `path` (the `PATH`
/// of the environment it will run in) when the app has recorded none.
pub fn resolve(codex_home: &Path, path: Option<&OsString>) -> Result<Launch, String> {
    discover(codex_home, path)?.ok_or_else(|| {
        "Codex is not installed on this machine: neither the ChatGPT desktop app nor a codex command was found".into()
    })
}

/// Selection is independent of executability: the app's path follows only
/// the app's own upgrades, and a path that leads nowhere is reported and
/// fails on use, never silently replaced by another installation.
pub fn discover(codex_home: &Path, path: Option<&OsString>) -> Result<Option<Launch>, String> {
    let config = read_config(codex_home)?;
    let mut computer_use = config.get("mcp_servers").and_then(|servers| servers.get("node_repl")).cloned();
    if let Some(recorded) = app_binary(codex_home, computer_use.as_ref())? {
        let Some((from, to)) = super::app_package::moved(&recorded) else {
            return Ok(Some(app_launch(recorded, computer_use, Vec::new())));
        };
        let binary = recorded.strip_prefix(&from).map_or_else(|_| recorded.clone(), |within| to.join(within));
        // Codex reads its Computer Use server from the same configuration:
        // its paths are given again for the installed version.
        let overrides = computer_use
            .as_mut()
            .map(|server| super::app_package::rebase(server, &from, &to, "mcp_servers.node_repl"))
            .unwrap_or_default();
        return Ok(Some(app_launch(binary, computer_use, overrides)));
    }
    let names: &[&str] = if cfg!(windows) { &["codex.exe", "codex.cmd"] } else { &["codex"] };
    let binary = path.and_then(|path| {
        std::env::split_paths(path)
            .find_map(|directory| names.iter().map(|name| directory.join(name)).find(|found| found.is_file()))
    });
    Ok(binary.map(|binary| Launch {
        binary,
        source: ProgramSource::Path,
        args: vec!["app-server".to_string()],
        computer_use,
        assisted_config: None,
    }))
}

/// The Codex to start for `codex_home`, as `discover` finds it, except that
/// a home the desktop app does not use, which therefore records no program
/// of the app's (the app writes it only into its own home), runs the app's
/// program recorded in `program_home`. It does not get the app's Computer
/// Use: that entry is the app's home's, and so is the desk it reaches.
pub fn discover_for(
    codex_home: &Path,
    program_home: Option<&Path>,
    path: Option<&OsString>,
) -> Result<Option<Launch>, String> {
    let own = discover(codex_home, path)?;
    if own.as_ref().is_some_and(|launch| launch.source == ProgramSource::DesktopApp) {
        return Ok(own);
    }
    let Some(program_home) = program_home.filter(|program_home| *program_home != codex_home) else { return Ok(own) };
    match discover(program_home, path)? {
        Some(app) if app.source == ProgramSource::DesktopApp => {
            Ok(Some(app_launch(app.binary, own.and_then(|own| own.computer_use), Vec::new())))
        }
        _ => Ok(own),
    }
}

/// The desktop app's program, started as the app starts it; `overrides`
/// are further `-c` settings.
fn app_launch(binary: PathBuf, computer_use: Option<toml::Value>, overrides: Vec<String>) -> Launch {
    let mut args = vec![
        "-c".to_string(),
        "features.code_mode_host=true".to_string(),
        // Both Computer Use services name their tool `js`, and Codex keeps
        // whichever registers first; only the Windows-native one stays.
        "-c".to_string(),
        "plugins.unified-computer-use@openai-bundled.mcp_servers.cua_repl.enabled=false".to_string(),
    ];
    for setting in overrides {
        args.extend(["-c".to_string(), setting]);
    }
    args.push("app-server".to_string());
    Launch { binary, source: ProgramSource::DesktopApp, args, computer_use, assisted_config: None }
}

/// `discover_for`, with nothing found an error.
pub fn resolve_for(codex_home: &Path, program_home: Option<&Path>, path: Option<&OsString>) -> Result<Launch, String> {
    discover_for(codex_home, program_home, path)?.ok_or_else(|| {
        "Codex is not installed on this machine: neither the ChatGPT desktop app nor a codex command was found".into()
    })
}

/// The `codex.exe` the desktop app recorded, including a path removed by an upgrade.
fn app_binary(codex_home: &Path, computer_use: Option<&toml::Value>) -> Result<Option<PathBuf>, String> {
    let path = codex_home.join("config.toml");
    let binary = computer_use.and_then(|server| server.get("env")).and_then(|env| env.get("CODEX_CLI_PATH"));
    match binary {
        None => Ok(None),
        Some(value) => value.as_str().filter(|value| !value.is_empty()).map(PathBuf::from).map(Some).ok_or_else(|| {
            format!("{}: mcp_servers.node_repl.env.CODEX_CLI_PATH must be a nonempty path", path.display())
        }),
    }
}
