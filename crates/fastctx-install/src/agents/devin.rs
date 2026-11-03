//! Devin Local and CLI 3000.11.234 share the current Desktop harness.
//! MCP lives in mcp_config.json, not the legacy Windsurf directory.
//! See https://docs.devin.ai/cli/extensibility/mcp/configuration.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Guidance, Home, Mcp, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "devin",
    title: "Devin Desktop / Local",
    home: if cfg!(windows) {
        Home { env: &[("APPDATA", "devin")], default: "AppData/Roaming/devin" }
    } else {
        Home { env: &[("XDG_CONFIG_HOME", "devin")], default: ".config/devin" }
    },
    marks: &[],
    programs: &[Program::Command("devin"), Program::Command("devin-desktop")],
    mcp: Mcp {
        to: To::One(Place { file: "mcp_config.json", format: Format::Jsonc, at: &["mcpServers"] }),
        slot: Slot::Named,
        entry,
    },
    guidance: Some(Guidance { file: "AGENTS.md", head: None, most: None }),
    shell: &[Edit::Add {
        to: To::One(Place { file: "config.json", format: Format::Jsonc, at: &["permissions", "deny"] }),
        item: || json!("exec"),
    }],
    limits: &[],
    data: None,
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env, "transport": "stdio" })
}
