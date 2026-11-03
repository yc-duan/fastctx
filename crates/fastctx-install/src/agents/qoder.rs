//! Qoder CLI 1.1.81 keeps user settings and AGENTS.md under QODER_CONFIG_DIR,
//! the `.qoder` its editor keeps too. Each of its shell tools is denied by
//! its own name; no pattern names them all without MCP's too.
//! See https://docs.qoder.com/cli/settings.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Guidance, Home, Mcp, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "qoder",
    title: "Qoder CLI",
    home: Home { env: &[("QODER_CONFIG_DIR", "")], default: ".qoder" },
    marks: &["settings.json", "AGENTS.md", ".auth"],
    programs: &[Program::Command("qodercli"), Program::Npm("@qoder-ai/qodercli")],
    mcp: Mcp {
        to: To::One(Place { file: "settings.json", format: Format::Jsonc, at: &["mcpServers"] }),
        slot: Slot::Named,
        entry,
    },
    guidance: Some(Guidance { file: "AGENTS.md", head: None, most: None }),
    shell: &[
        Edit::Add { to: DENY, item: || json!("Bash") },
        Edit::Add { to: DENY, item: || json!("PowerShell") },
        Edit::Add { to: DENY, item: || json!("Monitor") },
        Edit::Add { to: DENY, item: || json!("BashInput") },
        Edit::Add { to: DENY, item: || json!("BashOutput") },
        Edit::Add { to: DENY, item: || json!("BashResize") },
    ],
    limits: &[],
    data: None,
    identity: |_| None,
};

/// Where its tools are denied.
const DENY: To = To::One(Place { file: "settings.json", format: Format::Jsonc, at: &["permissions", "deny"] });

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}
