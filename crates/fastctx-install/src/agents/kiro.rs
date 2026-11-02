//! Kiro IDE 1.4.62 MCP and steering. A permissions.yaml file hides permissions.json,
//! so a static JSON edit cannot reliably disable its shell. Authentication is outside KIRO_HOME.
//! See https://kiro.dev/docs/mcp/configuration/ and https://kiro.dev/docs/permissions/.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Program, Server};

pub(super) const AGENT: Agent = Agent {
    name: "kiro",
    title: "Kiro IDE",
    home: Home { env: &[("KIRO_HOME", "")], default: ".kiro" },
    programs: &[Program::Command("kiro")],
    mcp: Mcp { file: "settings/mcp.json", format: Format::Json, at: &["mcpServers"], entry },
    guidance: Some("steering/fastctx.md"),
    shell: None,
    limits: &[],
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}
