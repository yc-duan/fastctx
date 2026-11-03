//! Kiro IDE 1.4.62 MCP and steering. Its permissions are read from
//! permissions.yaml where that is there, even empty, else from
//! permissions.json; a shell rule denying goes where they are read.
//! Authentication is outside KIRO_HOME.
//! See https://kiro.dev/docs/mcp/configuration/ and https://kiro.dev/docs/permissions/.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Guidance, Home, Mcp, Pick, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "kiro",
    title: "Kiro IDE",
    home: Home { env: &[("KIRO_HOME", "")], default: ".kiro" },
    marks: &[],
    programs: &[Program::Command("kiro")],
    mcp: Mcp {
        to: To::One(Place { file: "settings/mcp.json", format: Format::Json, at: &["mcpServers"] }),
        slot: Slot::Named,
        entry,
    },
    guidance: Some(Guidance { file: "steering/fastctx.md", head: None, most: None }),
    shell: &[Edit::Add {
        to: To::Among(PERMISSIONS, Pick::Present { made: 0 }),
        item: || json!({ "capability": "shell", "effect": "deny" }),
    }],
    limits: &[],
    data: None,
    identity: |_| None,
};

const PERMISSIONS: &[Place] = &[
    Place { file: "settings/permissions.yaml", format: Format::Yaml, at: &["rules"] },
    Place { file: "settings/permissions.json", format: Format::Json, at: &["rules"] },
];

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}
