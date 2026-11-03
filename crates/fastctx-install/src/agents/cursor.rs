//! Cursor 3.27.22 user-level MCP. Its shell is refused by a hook that
//! fails closed, which counts only in a hooks file with a version; a user
//! rule is always applied under its header's `alwaysApply`.
//! See https://cursor.com/docs/hooks and https://cursor.com/docs/context/mcp.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Guidance, Home, Mcp, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "cursor",
    title: "Cursor",
    home: Home { env: &[], default: ".cursor" },
    marks: &[],
    programs: &[Program::Command("cursor")],
    mcp: Mcp {
        to: To::One(Place { file: "mcp.json", format: Format::Jsonc, at: &["mcpServers"] }),
        slot: Slot::Named,
        entry,
    },
    guidance: Some(Guidance { file: "rules/fastctx.mdc", head: Some("---\nalwaysApply: true\n---\n"), most: None }),
    shell: &[
        Edit::Default {
            to: To::One(Place { file: "hooks.json", format: Format::Jsonc, at: &["version"] }),
            value: || json!(1),
        },
        Edit::Add {
            to: To::One(Place { file: "hooks.json", format: Format::Jsonc, at: &["hooks", "beforeShellExecution"] }),
            item: || json!({ "command": "exit 2", "failClosed": true }),
        },
    ],
    limits: &[],
    data: None,
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}
