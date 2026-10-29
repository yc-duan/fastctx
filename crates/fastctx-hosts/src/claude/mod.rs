//! Claude residents: sessions of Claude Code that the World hears through
//! its plugin (`world-colleagues.md`, 话怎样送到居民眼前).
//!
//! The plugin carries a mod, code that runs inside Claude Code: it reports
//! the session's turns and tool uses and hands the session what the World
//! has for it, through files the node reads and writes (`exchange`). The
//! node keeps the plugin in a local marketplace under the World's
//! directory, from which Claude Code loads it in place, so a new node's
//! plugin is the one the next session starts with. A background session the
//! World starts or wakes goes through the `claude` command (`background`).
//!
//! Uses Claude Code's public mod and plugin hook contracts, with command
//! knowledge pinned to the statically inspected 2.1.294 program.

mod accounting;
pub mod agents;
pub mod assisted;
pub mod background;
pub mod calls;
pub mod catalog;
mod content;
mod context;
pub mod control;
pub mod ending;
pub mod exchange;
pub mod lifecycle;
pub mod models;
mod oldest;
pub mod pricing;
pub mod projects;
pub mod startup;
pub mod subagents;
pub mod tail;
pub mod temp;
pub mod thoughts;
pub mod titles;
mod tools;
pub mod transcript;
pub mod trust;

pub use oldest::{OLDEST, supported};

use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use serde_json::json;

/// The marketplace's name, as `claude plugin` commands name it.
pub const MARKETPLACE: &str = "fastctx";
/// The plugin's name.
pub const PLUGIN: &str = "fastctx-world";

/// The version the World's plugin is written as: this fastctx's.
pub const PLUGIN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The permission rule allowing every tool of the plugin's `fastctx` server:
/// Claude Code names a plugin's MCP tools `mcp__plugin_<plugin>_<server>__…`.
/// Only this form matches a server's tools; `mcp__*` does not.
pub const WORLD_TOOLS: &str = "mcp__plugin_fastctx-world_fastctx__*";

/// Claude Code's limit on a tool answer, in tokens, set past the World's own
/// (`world-view.md`, 一次给多少): Claude Code estimates an answer as its
/// UTF-16 length over four, which for the World's at most 50,000 units is
/// at most 50,000, and passes an answer within half its limit as it is.
pub const MAX_MCP_OUTPUT_TOKENS: u64 = 100_000;

/// Claude Code's limit on an MCP tool's description, in characters, set to
/// what it allows a tool loaded through its tool search. By default it keeps
/// the first 4,096 characters of a tool always loaded, as `run` is, and `run`
/// says more than that: Claude would lose how to edit files, reach other
/// machines and combine the commands.
pub const MAX_MCP_DESCRIPTION_LENGTH: u64 = 16_384;

const MOD: &str = include_str!("world.js");

/// How long the mod keeps an answer to whether fastctx is being removed, and
/// how soon it tries again what failed; what the World has for a session
/// wakes the mod through its control helper instead. A removal waits two of
/// these after it marks the World, so every mod that read the marker has
/// stopped writing.
pub const MOD_LOOK: std::time::Duration = std::time::Duration::from_millis(2000);

/// How words the World says on its own account begin, as the model reads
/// them; the mod frames them the same way (`world.js`).
const FRAME: &str = "A message from fastctx, not typed by the user:";
/// The frame written before 2026-10-09 (UTC); session histories still hold it.
const FRAME_V1: &str = "The fastctx plugin delivers a message from the World.";
/// How the user's words sent into a running turn begin, as the model reads
/// them beside a tool's result (`world.js`, `userFramed`).
const USER_FRAME: &str = "The user sent a new message while you were working:";

/// Words the World says on its own account, as the model reads them.
pub fn framed(text: &str) -> String {
    format!("{FRAME}\n\n{text}")
}

/// Whether words a session heard went in framed by the World: its own
/// telling, or the user's words sent into a running turn. The World records
/// both as it hands them over. The session hears them inside Claude Code's
/// own frame for what a plugin submits, which puts a line of its own before
/// them (2.1.289).
pub fn is_framed(text: &str) -> bool {
    [FRAME, FRAME_V1, USER_FRAME].iter().any(|frame| text.starts_with(frame) || text.contains(&format!("\n{frame}\n")))
}

/// The words before the World's telling, where it went in beside them, as
/// the context of a prompt that starts a turn; `None` where it is all the
/// World's, or none of it is.
pub fn beside_framed(text: &str) -> Option<&str> {
    let frames = [FRAME, FRAME_V1, USER_FRAME];
    if frames.iter().any(|frame| text.starts_with(frame)) {
        return None;
    }
    let at = frames.iter().filter_map(|frame| text.find(&format!("\n{frame}\n"))).min()?;
    let before = text[..at].trim_end();
    // Claude Code's own line before what a plugin submits says nothing.
    let plugin_line = !before.contains('\n') && before.starts_with("The ") && before.contains(" plugin sent a message");
    (!before.is_empty() && !plugin_line).then_some(before)
}

/// The local marketplace: `<world>/claude-plugins`.
pub fn marketplace(world: &Path) -> PathBuf {
    world.join("claude-plugins")
}

/// The plugin's own directory, as `--plugin-dir` takes it.
pub fn plugin_dir(world: &Path) -> PathBuf {
    marketplace(world).join(PLUGIN)
}

/// Writes the marketplace and plugin using this World's stable program.
/// Each file is replaced whole; the caller cannot select a delivery image.
pub fn write_plugin(world: &Path) -> io::Result<PathBuf> {
    let root =
        trust::directory_key(world.parent().ok_or_else(|| io::Error::other("the World's directory has no parent"))?)?;
    let command = format!("{root}/bin/fastctx{}", if cfg!(windows) { ".exe" } else { "" });
    let market = marketplace(world);
    let plugin = plugin_dir(world);
    let version = PLUGIN_VERSION;
    let description = "The World: its tools, and word of what happens in it, for every Claude Code session";
    // The plugin belongs to this World, and its server says so itself: a
    // background session's environment is the Claude Code daemon's, set by
    // whoever started the daemon, so it cannot be trusted to name the World.
    let files = [
        (
            market.join(".claude-plugin").join("marketplace.json"),
            json!({
                "name": MARKETPLACE,
                "owner": { "name": "FastCtx" },
                "plugins": [{ "name": PLUGIN, "source": format!("./{PLUGIN}"), "description": description }],
            }),
        ),
        (
            plugin.join(".claude-plugin").join("plugin.json"),
            json!({
                "name": PLUGIN,
                "version": version,
                "description": description,
                "author": { "name": "FastCtx" },
                "mcpServers": {
                    "fastctx": { "command": command, "args": ["serve"], "env": {
                        "FASTCTX_HOME": root,
                        "FASTCTX_AGENT_HOME": "${CLAUDE_CONFIG_DIR:-}",
                    } },
                },
            }),
        ),
        (
            plugin.join("hooks").join("hooks.json"),
            json!({
                "modules": ["./world.js"],
                "hooks": { "Elicitation": [{ "hooks": [{
                    "type": "command",
                    "command": command,
                    "args": ["claude-elicitation", world],
                }] }] },
            }),
        ),
    ];
    for (path, value) in files {
        let mut bytes = serde_json::to_vec_pretty(&value).expect("JSON values serialize");
        bytes.push(b'\n');
        write(&path, &bytes)?;
    }
    // The World's directory goes into the mod as a JavaScript string: its
    // physical path, as Claude Code itself spells directories (long names,
    // forward slashes, no `\\?\` prefix). A mod given a Windows short name
    // such as `MOTORW~1` writes nothing at all.
    let directory = trust::directory_key(world)?;
    let module = MOD
        .replace("__FASTCTX_CONTROLS__", include_str!("control.js"))
        .replace("__FASTCTX_COMMANDS__", include_str!("commands.js"))
        .replace("__FASTCTX_NATIVE_GOAL__", include_str!("native_goal.js"))
        .replace("__FASTCTX_COMMAND_CATALOG__", include_str!("command_catalog.json"))
        .replace("__FASTCTX_ASSISTED__", include_str!("assisted.js"))
        .replace("__FASTCTX_ACCOUNT_AGENTS__", include_str!("agent_config.js"))
        .replace("__FASTCTX_SELECTION__", include_str!("selection.js"))
        .replace("__FASTCTX_REWIND__", include_str!("rewind.js"))
        .replace("__FASTCTX_OBSERVATIONS__", include_str!("observations.js"))
        .replace("__FASTCTX_USAGE__", include_str!("accounting.js"))
        .replace("__FASTCTX_FAILURES__", include_str!("failure.js"))
        .replace("__FASTCTX_TURN_FAILURES__", include_str!("turn_failures.js"))
        .replace("__FASTCTX_GOAL__", include_str!("goal.js"));
    let module = module.replace("__FASTCTX_WORLD__", &serde_json::to_string(&directory).expect("strings serialize"));
    let module = module.replace("__FASTCTX_COMMAND__", &serde_json::to_string(&command).expect("strings serialize"));
    let module = module.replace("__FASTCTX_LOOK_MS__", &MOD_LOOK.as_millis().to_string());
    write(&plugin.join("hooks").join("presentation.js"), include_bytes!("presentation.js"))?;
    write(&plugin.join("hooks").join("steps.js"), include_bytes!("steps.js"))?;
    write(&plugin.join("hooks").join("world.js"), module.as_bytes())?;
    write(&plugin.join("hooks").join("world.test.ts"), include_bytes!("world.test.ts"))?;
    write(&plugin.join("hooks").join("control.test.ts"), include_bytes!("control.test.ts"))?;
    Ok(plugin)
}

/// Writes a file of the plugin unless it already holds `bytes`: a running
/// Claude Code reloads the plugin whenever one of its files changes.
fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if std::fs::read(path).is_ok_and(|held| held == bytes) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    publish(path, bytes, Permissions::Private)
}
