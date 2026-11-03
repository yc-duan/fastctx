//! Qoder IDE 1.38.0 uses ~/.qoder/mcp.json, independently of the CLI's settings.
//! QODER_CONFIG_DIR and the CLI's global instructions are not assumed to apply.
//! See https://docs.qoder.com/user-guide/chat/model-context-protocol.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Server};

pub(super) const AGENT: Agent = Agent {
    name: "qoder-ide",
    title: "Qoder IDE",
    home: Home { env: &[], default: ".qoder" },
    // The CLI also installs a command named qoder, so it cannot identify the IDE.
    programs: &[],
    mcp: Mcp { file: "mcp.json", format: Format::Json, at: &["mcpServers"], entry },
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
