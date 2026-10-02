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
/// The local marketplace that holds the plugin, so a user-level install can name it as
/// `fastctx-world@fastctx`. A plugin with a relative source in a local-directory marketplace
/// loads in place, so a new node build's plugin takes effect at the next session start.
const MARKETPLACE_NAME: &str = "fastctx";

/// Environment variable Claude Code gives each session's inbox; its value is a path that
/// differs per session, which is what pairs the mod's registration with the session's MCP
/// connection. Only the address is read, never the token beside it.
pub(crate) const INBOX_ADDRESS_VARIABLE: &str = "CLAUDE_CODE_MESSAGING_SOCKET";

/// Writes the marketplace and the plugin under `<world>/claude-plugins/`.
pub(crate) fn write_plugin(world_dir: &Path) -> Result<(), String> {
    let world = world_dir.to_string_lossy().replace('\\', "/");
    let dir = plugin_dir(world_dir);
    let files: [(PathBuf, String); 4] = [
        (
            marketplace_dir(world_dir).join(".claude-plugin").join("marketplace.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": MARKETPLACE_NAME,
                "owner": {"name": "FastCtx"},
                "plugins": [{
                    "name": PLUGIN_NAME,
                    "source": format!("./{PLUGIN_NAME}"),
                    "description": "FastCtx World: tells the World this session exists and delivers its messages.",
                }],
            }))
            .expect("a literal serializes"),
        ),
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

/// The local marketplace the node keeps: `<world>/claude-plugins`.
pub(crate) fn marketplace_dir(world_dir: &Path) -> PathBuf {
    world_dir.join("claude-plugins")
}

/// The plugin itself, inside the marketplace; background residents load it with `--plugin-dir`.
pub(crate) fn plugin_dir(world_dir: &Path) -> PathBuf {
    marketplace_dir(world_dir).join(PLUGIN_NAME)
}

/// The official commands that install the plugin for every Claude Code session of the user:
/// add the local marketplace, then install the plugin from it at user scope. `fastctx apply`
/// runs them; Claude Code itself does the installing.
pub(crate) fn install_commands(world_dir: &Path) -> [Vec<String>; 2] {
    [
        vec!["plugin".into(), "marketplace".into(), "add".into(), marketplace_dir(world_dir).to_string_lossy().into_owned()],
        vec!["plugin".into(), "install".into(), format!("{PLUGIN_NAME}@{MARKETPLACE_NAME}"), "--scope".into(), "user".into()],
    ]
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

/// A new session id in the UUID form `--session-id` requires.
pub(crate) fn new_session_id() -> Result<String, String> {
    let mut bytes = crate::world::crypto::random_bytes::<16>()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    Ok(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

/// What a background Claude resident is started from.
pub(crate) struct Background<'a> {
    pub(crate) resident: &'a str,
    pub(crate) session: &'a str,
    /// The town's directory, or the outside; Claude Code must trust it before `--bg` works.
    pub(crate) cwd: &'a Path,
    /// The resident's records; its launch files are kept beside them.
    pub(crate) records: &'a Path,
    pub(crate) world_dir: &'a Path,
    pub(crate) fastctx: &'a Path,
    pub(crate) instructions: &'a str,
    pub(crate) words: &'a str,
}

/// Starts `claude --bg` for a resident and returns once Claude Code has taken the session. The
/// session runs under Claude Code's own supervisor; the World talks to it through the mod.
pub(crate) async fn start_background(background: &Background<'_>) -> Result<(), String> {
    let dir = background.records.join("claude");
    std::fs::create_dir_all(&dir).map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(&dir)))?;
    let mcp_config = dir.join("mcp.json");
    let settings = dir.join("settings.json");
    let prompt = dir.join("instructions.md");
    let config = serde_json::json!({
        "mcpServers": {"fastctx": {
            "command": background.fastctx,
            "args": ["serve"],
            "env": {"FASTCTX_RESIDENT": background.resident},
        }}
    });
    let encode = |value: &serde_json::Value| serde_json::to_vec_pretty(value).expect("JSON values encode");
    crate::world::write_atomic(&mcp_config, &encode(&config))?;
    crate::world::write_atomic(&settings, &encode(&resident_settings()))?;
    crate::world::write_atomic(&prompt, background.instructions.as_bytes())?;
    let plugin_dir = plugin_dir(background.world_dir);
    let args = launch_args(&Launch {
        session: background.session,
        plugin_dir: &plugin_dir,
        mcp_config: &mcp_config,
        settings: &settings,
        system_prompt_file: &prompt,
        first_words: background.words,
    });
    // The session's environment names its resident, which the mod reports when it registers.
    claude(&args, Some(background.cwd), Some(background.resident)).await.map(|_| ())
}

fn claude_binary() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd"] } else { &["claude"] };
    std::env::split_paths(&paths).flat_map(|dir| names.iter().map(move |name| dir.join(name))).find(|path| path.is_file())
}

/// Whether Claude Code's supervisor still runs a background session's process. A session idle
/// for about an hour has its process stopped while its conversation stays on disk; words for
/// it then wake it rather than wait in an inbox nobody reads. `claude agents --json --all`
/// lists every background session with the process it runs in, if any.
pub(crate) async fn background_running(session: &str) -> Result<bool, String> {
    let output = claude(&["agents".into(), "--json".into(), "--all".into()], None, None).await?;
    let agents: serde_json::Value = serde_json::from_slice(&output)
        .map_err(|error| format!("claude agents --json answered with something that is not JSON: {error}"))?;
    Ok(agents
        .as_array()
        .into_iter()
        .flatten()
        .find(|agent| agent.get("sessionId").and_then(serde_json::Value::as_str) == Some(session))
        .and_then(|agent| agent.get("pid"))
        .is_some_and(|pid| pid.as_u64().is_some()))
}

/// Wakes a stopped background session with words, which arrive as the user's prompt
/// (`claude --resume <session> --bg "<words>"`, nothing else, or Claude Code starts a copy).
pub(crate) async fn wake(session: &str, words: &str, cwd: &Path) -> Result<(), String> {
    claude(&wake_args(session, words), Some(cwd), None).await.map(|_| ())
}

/// Runs the claude command and returns its standard output, or its complaint. `resident`
/// names the resident in the environment of a session it starts.
async fn claude(args: &[String], cwd: Option<&Path>, resident: Option<&str>) -> Result<Vec<u8>, String> {
    let binary = claude_binary().ok_or_else(|| "Claude Code is not installed on this machine: no claude command was found.".to_string())?;
    let mut command = tokio::process::Command::new(&binary);
    command.args(args).stdin(std::process::Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if let Some(resident) = resident {
        command.env(crate::town::session::RESIDENT_VARIABLE, resident);
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command
        .output()
        .await
        .map_err(|error| format!("Cannot start {}: {error}", crate::paths::display_path(&binary)))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(format!(
        "claude {} failed ({}): {}{}",
        args.first().map(String::as_str).unwrap_or_default(),
        output.status,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}
