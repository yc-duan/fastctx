//! Copilot CLI 1.0.105 user-level MCP and instructions.
//! User command hooks fail open on timeout, so they are not advertised as disabling the shell.
//! See https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Program, Server};

pub(super) const AGENT: Agent = Agent {
    name: "copilot",
    title: "GitHub Copilot CLI",
    home: Home { env: &[("COPILOT_HOME", "")], default: ".copilot" },
    programs: &[Program::Command("copilot"), Program::Npm("@github/copilot")],
    mcp: Mcp { file: "mcp-config.json", format: Format::Json, at: &["mcpServers"], entry },
    guidance: Some("copilot-instructions.md"),
    shell: None,
    limits: &[],
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    // As `copilot mcp add` writes one: a local server with all its tools.
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "type": "local", "command": server.command, "args": server.args, "env": env, "tools": ["*"] })
}
