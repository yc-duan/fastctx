//! Cursor 3.27.22 user-level MCP. Hooks require both a version and a rule;
//! its always-on rule files require frontmatter. Neither is a single plain edit.
//! See https://cursor.com/docs/hooks and https://cursor.com/docs/context/mcp.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Program, Server};

pub(super) const AGENT: Agent = Agent {
    name: "cursor",
    title: "Cursor",
    home: Home { env: &[], default: ".cursor" },
    programs: &[Program::Command("cursor")],
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
