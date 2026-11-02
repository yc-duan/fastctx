//! Claude Desktop 1.4758 user-data configuration, separate from Claude Code.
//! No stable local instruction file or user shell switch was established for Desktop Chat.
//! Its command detection deliberately does not match Claude Code.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Server};

pub(super) const AGENT: Agent = Agent {
    name: "claude-desktop",
    title: "Claude Desktop",
    home: if cfg!(windows) {
        Home { env: &[("APPDATA", "Claude")], default: "AppData/Roaming/Claude" }
    } else if cfg!(target_os = "macos") {
        Home { env: &[], default: "Library/Application Support/Claude" }
    } else {
        Home { env: &[("XDG_CONFIG_HOME", "Claude")], default: ".config/Claude" }
    },
    programs: &[],
    mcp: Mcp { file: "claude_desktop_config.json", format: Format::Json, at: &["mcpServers"], entry },
    guidance: None,
    shell: None,
    limits: &[],
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}
