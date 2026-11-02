//! Continue IDE global JSON MCP loader at revision 5522c6f4.
//! The CLI has a different YAML-only loader. Rule activation outside the default
//! .continue path needs frontmatter that the plain guidance writer does not supply.
//! See core/context/mcp/json/loadJsonMcpConfigs.ts in https://github.com/continuedev/continue.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Server};

pub(super) const AGENT: Agent = Agent {
    name: "continue-ide",
    title: "Continue (IDE)",
    home: Home { env: &[("CONTINUE_GLOBAL_DIR", "")], default: ".continue" },
    programs: &[],
    mcp: Mcp { file: "mcpServers/fastctx.json", format: Format::Jsonc, at: &["mcpServers"], entry },
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
