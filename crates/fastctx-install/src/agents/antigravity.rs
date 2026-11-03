//! Antigravity App 2.28.2 and CLI 1.4.12 share MCP and permission grants.
//! The IDE has not been verified against the shared permission file.
//! See https://antigravity.google/docs/permissions and the captured App fixture.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Guidance, Home, Mcp, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "antigravity",
    title: "Antigravity App / CLI",
    home: Home { env: &[], default: ".gemini" },
    marks: &[],
    programs: &[Program::Command("agy"), Program::Command("antigravity")],
    mcp: Mcp {
        to: To::One(Place { file: "config/mcp_config.json", format: Format::Json, at: &["mcpServers"] }),
        slot: Slot::Named,
        entry,
    },
    // One instructions file is read up to 24,000 bytes.
    guidance: Some(Guidance { file: "AGENTS.md", head: None, most: Some(24_000) }),
    shell: &[Edit::Add {
        to: To::One(Place {
            file: "config/config.json",
            format: Format::Json,
            at: &["userSettings", "globalPermissionGrants", "deny"],
        }),
        item: || json!("command(*)"),
    }],
    limits: &[],
    data: None,
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}
