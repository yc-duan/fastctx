//! Trae IDE 3.6.13 stores MCP beside its user settings, not in ~/.trae.
//! Rules and authentication are not inferred from this editor-profile root.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Program, Server};

pub(super) const AGENT: Agent = Agent {
    name: "trae",
    title: "Trae IDE",
    home: if cfg!(windows) {
        Home { env: &[("APPDATA", "Trae/User")], default: "AppData/Roaming/Trae/User" }
    } else if cfg!(target_os = "macos") {
        Home { env: &[], default: "Library/Application Support/Trae/User" }
    } else {
        Home { env: &[("XDG_CONFIG_HOME", "Trae/User")], default: ".config/Trae/User" }
    },
    programs: &[Program::Command("trae")],
    mcp: Mcp { file: "mcp.json", format: Format::Jsonc, at: &["mcpServers"], entry },
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
