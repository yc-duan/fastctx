//! Qoder IDE 1.38.0 uses ~/.qoder/mcp.json, independently of the CLI's settings.
//! QODER_CONFIG_DIR and the CLI's global instructions are not assumed to apply.
//! See https://docs.qoder.com/user-guide/chat/model-context-protocol.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Place, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "qoder-ide",
    title: "Qoder IDE",
    home: Home { env: &[], default: ".qoder" },
    // The CLI also installs a command named qoder, so it cannot identify the IDE.
    // What the editor makes at its first start, beside the CLI's files.
    marks: &["argv.json", "extensions", "mcp.json"],
    programs: &[],
    mcp: Mcp {
        to: To::One(Place { file: "mcp.json", format: Format::Json, at: &["mcpServers"] }),
        slot: Slot::Named,
        entry,
    },
    guidance: None,
    shell: &[],
    limits: &[],
    data: None,
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}
