//! Qoder CLI 1.1.81 keeps user settings and AGENTS.md under QODER_CONFIG_DIR.
//! Its Bash, PowerShell and terminal permissions require separate deny rules.
//! See https://docs.qoder.com/cli/settings.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Program, Server};

pub(super) const AGENT: Agent = Agent {
    name: "qoder",
    title: "Qoder CLI",
    home: Home { env: &[("QODER_CONFIG_DIR", "")], default: ".qoder" },
    programs: &[Program::Command("qodercli"), Program::Npm("@qoder-ai/qodercli")],
    mcp: Mcp { file: "settings.json", format: Format::Jsonc, at: &["mcpServers"], entry },
    guidance: Some("AGENTS.md"),
    shell: None,
    limits: &[],
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}
