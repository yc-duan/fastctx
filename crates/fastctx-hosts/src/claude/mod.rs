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
//! Built against the mod API as measured on Claude Code 2.1.287 and run
//! against a stand-in host only; it has not yet run inside Claude Code.

pub mod background;
pub mod exchange;
pub mod trust;

use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use serde_json::json;

/// The marketplace's name, as `claude plugin` commands name it.
pub const MARKETPLACE: &str = "fastctx";
/// The plugin's name.
pub const PLUGIN: &str = "fastctx-world";

/// The permission rule allowing every tool of the plugin's `fastctx` server:
/// Claude Code names a plugin's MCP tools `mcp__plugin_<plugin>_<server>__…`.
/// Only this form matches a server's tools; `mcp__*` does not.
pub const WORLD_TOOLS: &str = "mcp__plugin_fastctx-world_fastctx__*";

/// Claude Code's limit on a tool answer, in tokens, set past the World's own
/// (`world-view.md`, 一次给多少): the World gives at most 150,000
/// characters, and every model is assumed to have a window of at least
/// 200,000 tokens, which is where this limit stops mattering.
pub const MAX_MCP_OUTPUT_TOKENS: u64 = 200_000;

const MOD: &str = include_str!("world.js");

/// The local marketplace: `<world>/claude-plugins`.
pub fn marketplace(world: &Path) -> PathBuf {
    world.join("claude-plugins")
}

/// The plugin's own directory, as `--plugin-dir` takes it.
pub fn plugin_dir(world: &Path) -> PathBuf {
    marketplace(world).join(PLUGIN)
}

/// Writes the marketplace and the plugin for the World in `world`, whose
/// tools `fastctx` serves. Each file is replaced whole.
pub fn write_plugin(world: &Path, fastctx: &Path) -> io::Result<PathBuf> {
    let market = marketplace(world);
    let plugin = plugin_dir(world);
    let version = env!("CARGO_PKG_VERSION");
    let description = "The World: its tools, and word of what happens in it, for every Claude Code session";
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
                "mcpServers": { "fastctx": { "command": fastctx, "args": ["serve"] } },
            }),
        ),
        (plugin.join("hooks").join("hooks.json"), json!({ "modules": ["./world.js"] })),
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
    let module = MOD.replace("__FASTCTX_WORLD__", &serde_json::to_string(&directory).expect("strings serialize"));
    let module = module
        .replace("__FASTCTX_COMMAND__", &serde_json::to_string(&fastctx.to_string_lossy()).expect("strings serialize"));
    write(&plugin.join("hooks").join("world.js"), module.as_bytes())?;
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
