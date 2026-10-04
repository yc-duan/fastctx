//! Which Codex to start, and how (`world-colleagues.md`, 世界怎样拉起 GPT
//! 同事).
//!
//! The ChatGPT desktop app records its own `codex.exe` in the user's Codex
//! configuration, as the `CODEX_CLI_PATH` of its Computer Use server
//! (`[mcp_servers.node_repl.env]`). That binary, started with the flags the
//! app uses, lends every thread the app's Computer Use, plugins and
//! connectors. The app moves the binary when it upgrades and renames the
//! Computer Use pipe when it restarts, rewriting the configuration each
//! time, so the configuration is read again before every start. Without the
//! app, a standalone `codex` on the search path speaks the same protocol.

use fastctx_types::agents::ProgramSource;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A Codex to start: the binary, its arguments, and the user's
/// configuration as it was read, to tell later whether it changed.
#[derive(Clone, Debug)]
pub struct Launch {
    pub binary: PathBuf,
    pub source: ProgramSource,
    pub args: Vec<String>,
    pub config: Option<Vec<u8>>,
}

/// The user's Codex directory: `CODEX_HOME` when it is set, as Codex reads
/// it, and `.codex` in the home directory otherwise.
pub fn codex_home(home: &Path, env: &[(OsString, OsString)]) -> PathBuf {
    match env.iter().find(|(name, _)| name == "CODEX_HOME").map(|(_, value)| value) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(".codex"),
    }
}

/// The user's `config.toml` as it is now; `None` when there is none.
pub fn read_config(codex_home: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = codex_home.join("config.toml");
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("reading {}: {error}", path.display())),
    }
}

/// The Codex to start now, looking for the program on `path` (the `PATH`
/// of the environment it will run in) when the app has recorded none.
pub fn resolve(codex_home: &Path, path: Option<&OsString>) -> Result<Launch, String> {
    discover(codex_home, path)?.ok_or_else(|| {
        "Codex is not installed on this machine: neither the ChatGPT desktop app nor a codex command was found".into()
    })
}

/// Selection is independent of executability: a stale app path must be
/// reported and fail on use, never silently select another installation.
pub fn discover(codex_home: &Path, path: Option<&OsString>) -> Result<Option<Launch>, String> {
    let config = read_config(codex_home)?;
    if let Some(binary) = app_binary(codex_home, config.as_deref())? {
        return Ok(Some(Launch {
            binary,
            source: ProgramSource::DesktopApp,
            args: vec![
                "-c".to_string(),
                "features.code_mode_host=true".to_string(),
                // Both Computer Use services name their tool `js`, and Codex
                // keeps whichever registers first; only the Windows-native
                // one stays.
                "-c".to_string(),
                "plugins.unified-computer-use@openai-bundled.mcp_servers.cua_repl.enabled=false".to_string(),
                "app-server".to_string(),
            ],
            config,
        }));
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
        config,
    }))
}

/// The `codex.exe` the desktop app recorded, including a path removed by an upgrade.
fn app_binary(codex_home: &Path, config: Option<&[u8]>) -> Result<Option<PathBuf>, String> {
    let Some(bytes) = config else {
        return Ok(None);
    };
    let path = codex_home.join("config.toml");
    let text = std::str::from_utf8(bytes).map_err(|error| format!("{} is not UTF-8: {error}", path.display()))?;
    let document: toml::Table =
        text.parse().map_err(|error: toml::de::Error| format!("reading {}: {}", path.display(), error.message()))?;
    let binary = document
        .get("mcp_servers")
        .and_then(|servers| servers.get("node_repl"))
        .and_then(|server| server.get("env"))
        .and_then(|env| env.get("CODEX_CLI_PATH"));
    match binary {
        None => Ok(None),
        Some(value) => value.as_str().filter(|value| !value.is_empty()).map(PathBuf::from).map(Some).ok_or_else(|| {
            format!("{}: mcp_servers.node_repl.env.CODEX_CLI_PATH must be a nonempty path", path.display())
        }),
    }
}
