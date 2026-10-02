//! Claude residents: the World's Claude Code plugin, the files its mod and the node share, and
//! the command lines that start or wake a background session.
//!
//! The plugin carries a mod (`world.js`) that runs inside Claude Code. It and the node talk only
//! through files under `<world>/claude/<session id>/` (the layout is described at the top of
//! `world.js`), so the mod needs nothing but file access, and the node never touches Claude
//! Code's process, its inbox token, or any credential.
//!
//! Claude Code was deliberately not run while this was built (the user's account is not used for
//! tests): the mod was exercised against a fake host shaped after the mod API measured on
//! 2026-10-02, and the command lines follow the CLI reference. Running them for real is still
//! to be done.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const WORLD_JS: &str = include_str!("world.js");
const PLUGIN_NAME: &str = "fastctx-world";

/// Environment variable Claude Code gives each session's inbox; its value is a path that
/// differs per session, which is what pairs the mod's registration with the session's MCP
/// connection. Only the address is read, never the token beside it.
pub(crate) const INBOX_ADDRESS_VARIABLE: &str = "CLAUDE_CODE_MESSAGING_SOCKET";

/// Writes the plugin into `dir` for a World whose directory is `world_dir`.
pub(crate) fn write_plugin(dir: &Path, world_dir: &Path) -> Result<(), String> {
    let world = world_dir.to_string_lossy().replace('\\', "/");
    let files: [(PathBuf, String); 3] = [
        (
            dir.join(".claude-plugin").join("plugin.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": PLUGIN_NAME,
                "version": env!("CARGO_PKG_VERSION"),
                "description": "FastCtx World: tells the World this session exists and delivers its messages.",
                "author": {"name": "FastCtx"},
            }))
            .expect("a literal serializes"),
        ),
        (dir.join("hooks").join("hooks.json"), r#"{ "modules": ["./world.js"] }"#.to_string()),
        (dir.join("hooks").join("world.js"), WORLD_JS.replace("__FASTCTX_WORLD_DIR__", &world)),
    ];
    for (path, text) in files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(parent)))?;
        }
        crate::world::write_atomic(&path, text.as_bytes())?;
    }
    Ok(())
}

/// Where the node keeps the plugin: `<world>/claude-plugin`.
pub(crate) fn plugin_dir(world_dir: &Path) -> PathBuf {
    world_dir.join("claude-plugin")
}

/// `<world>/claude/<session>`.
pub(crate) fn session_dir(world_dir: &Path, session: &str) -> PathBuf {
    world_dir.join("claude").join(session)
}

/// A delivery as the mod reads it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ModDelivery {
    pub(crate) id: String,
    pub(crate) from: String,
    pub(crate) kind: String,
    pub(crate) text: String,
    /// Only the user's own words, typed on one of the user's devices, go in as the user.
    #[serde(default)]
    pub(crate) as_user: bool,
}

/// Puts a delivery in the session's inbox; the mod hands it over and writes a receipt.
pub(crate) fn deliver(world_dir: &Path, session: &str, delivery: &ModDelivery) -> Result<(), String> {
    let inbox = session_dir(world_dir, session).join("inbox");
    std::fs::create_dir_all(&inbox).map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(&inbox)))?;
    let bytes = serde_json::to_vec(delivery).map_err(|error| format!("Cannot encode a delivery: {error}"))?;
    crate::world::write_atomic(&inbox.join(format!("{}.json", delivery.id)), &bytes)
}

/// What the mod wrote when its session started.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Registration {
    pub(crate) session: String,
    #[serde(default)]
    pub(crate) resident: Option<String>,
    #[serde(default)]
    pub(crate) inbox_address: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) surfaces: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) version: Option<serde_json::Value>,
}

/// One event file the mod wrote.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ModEvent {
    pub(crate) kind: String,
    pub(crate) at: String,
    #[serde(default)]
    pub(crate) aborted: Option<bool>,
    #[serde(default)]
    pub(crate) answer: Option<String>,
    #[serde(default)]
    pub(crate) tool: Option<String>,
}

/// Everything new under one session's directory since the last look: its events (removed once
/// read) and the deliveries it confirmed (the delivery and its receipt are removed).
pub(crate) struct Harvest {
    pub(crate) events: Vec<ModEvent>,
    pub(crate) delivered: Vec<String>,
}

pub(crate) fn registration(world_dir: &Path, session: &str) -> Result<Option<Registration>, String> {
    let path = session_dir(world_dir, session).join("session.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("Cannot parse {}: {error}", crate::paths::display_path(&path))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot read {}: {error}", crate::paths::display_path(&path))),
    }
}

/// Every session directory the mod created.
pub(crate) fn sessions(world_dir: &Path) -> Vec<String> {
    std::fs::read_dir(world_dir.join("claude"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("session.json").is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect()
}

pub(crate) fn harvest(world_dir: &Path, session: &str) -> Result<Harvest, String> {
    let dir = session_dir(world_dir, session);
    let mut events = Vec::new();
    let mut names = list(&dir.join("events"))?;
    names.sort();
    for name in names {
        let path = dir.join("events").join(&name);
        let bytes = std::fs::read(&path).map_err(|error| format!("Cannot read {}: {error}", crate::paths::display_path(&path)))?;
        match serde_json::from_slice::<ModEvent>(&bytes) {
            Ok(event) => events.push(event),
            // A file the mod is still writing parses on the next look.
            Err(_) if bytes.is_empty() => continue,
            Err(error) => return Err(format!("{} is not a mod event ({error})", crate::paths::display_path(&path))),
        }
        std::fs::remove_file(&path).map_err(|error| format!("Cannot remove {}: {error}", crate::paths::display_path(&path)))?;
    }
    let mut delivered = Vec::new();
    for id in list(&dir.join("receipts"))? {
        let delivery = dir.join("inbox").join(format!("{id}.json"));
        if delivery.exists() {
            std::fs::remove_file(&delivery)
                .map_err(|error| format!("Cannot remove {}: {error}", crate::paths::display_path(&delivery)))?;
        }
        let receipt = dir.join("receipts").join(&id);
        std::fs::remove_file(&receipt).map_err(|error| format!("Cannot remove {}: {error}", crate::paths::display_path(&receipt)))?;
        delivered.push(id);
    }
    Ok(Harvest { events, delivered })
}

fn list(dir: &Path) -> Result<Vec<String>, String> {
    match std::fs::read_dir(dir) {
        Ok(entries) => Ok(entries.flatten().filter_map(|entry| entry.file_name().to_str().map(str::to_string)).collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("Cannot list {}: {error}", crate::paths::display_path(dir))),
    }
}

/// What a background Claude resident is started with (`specs/world-projects.md`, the table in
/// "世界请居民进来").
pub(crate) struct Launch<'a> {
    pub(crate) session: &'a str,
    pub(crate) plugin_dir: &'a Path,
    pub(crate) mcp_config: &'a Path,
    pub(crate) settings: &'a Path,
    pub(crate) system_prompt_file: &'a Path,
    pub(crate) first_words: &'a str,
}

/// `claude --bg …` for a new background resident; the caller runs it in the town's directory
/// with `FASTCTX_RESIDENT` in the environment.
pub(crate) fn launch_args(launch: &Launch<'_>) -> Vec<String> {
    let path = |path: &Path| path.to_string_lossy().into_owned();
    vec![
        "--bg".into(),
        "--session-id".into(),
        launch.session.into(),
        "--plugin-dir".into(),
        path(launch.plugin_dir),
        "--mcp-config".into(),
        path(launch.mcp_config),
        "--strict-mcp-config".into(),
        "--settings".into(),
        path(launch.settings),
        "--append-system-prompt-file".into(),
        path(launch.system_prompt_file),
        launch.first_words.into(),
    ]
}

/// `claude --resume <session> --bg "<words>"`: wakes a background resident whose process the
/// supervisor stopped. Nothing else may be passed, or Claude Code starts a copy.
pub(crate) fn wake_args(session: &str, words: &str) -> Vec<String> {
    vec!["--resume".into(), session.into(), "--bg".into(), words.into()]
}

/// Settings for a background resident: the host's own read, write, search and shell tools are
/// off and the World's tools are allowed, so the session never stops on an approval.
pub(crate) fn resident_settings() -> serde_json::Value {
    serde_json::json!({
        "permissions": {
            "allow": ["mcp__fastctx"],
            "deny": ["Bash", "Read", "Write", "Edit", "MultiEdit", "NotebookEdit", "Grep", "Glob"],
        }
    })
}
