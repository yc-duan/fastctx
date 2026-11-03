//! ZCode 3.17, Z.ai's desktop app with its CLI inside: its own MCP servers
//! are under `mcp.servers` in `~/.zcode/cli/config.json`, and while that
//! holds any server, enabled or not, the `mcpServers` of the shared
//! `~/.agents/mcp.json` are not read; the two are never merged. Both are
//! strict JSON. Its permission service allows a tool before it looks at
//! those taken away, so its shell stays on. Its credentials are encrypted.
//! See https://zcode.z.ai/en/docs/mcp-services.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Guidance, Home, Mcp, Pick, Place, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "zcode",
    title: "ZCode",
    home: Home { env: &[], default: ".zcode" },
    marks: &[],
    programs: &[],
    mcp: Mcp { to: To::Among(SERVERS, Pick::Holding), slot: Slot::Named, entry },
    guidance: Some(Guidance { file: "AGENTS.md", head: None, most: Some(100 * 1024) }),
    shell: &[],
    limits: &[],
    data: None,
    identity: |_| None,
};

const SERVERS: &[Place] = &[
    Place { file: "cli/config.json", format: Format::Json, at: &["mcp", "servers"] },
    Place { file: "../.agents/mcp.json", format: Format::Json, at: &["mcpServers"] },
];

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}
