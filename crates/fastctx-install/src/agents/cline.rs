//! Cline 3.0.74 standard CLINE_DIR layout; separately redirected files are not inferred.
//! Its providers can hold several identities, with no single account implied here.
//! See https://github.com/cline/cline/tree/5b454165353ac1edc9a7db0ff416739044985908/sdk.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Guidance, Home, Mcp, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "cline",
    title: "Cline",
    home: Home { env: &[("CLINE_DIR", "")], default: ".cline" },
    marks: &[],
    programs: &[Program::Command("cline"), Program::Npm("cline")],
    mcp: Mcp {
        to: To::One(Place { file: "data/settings/cline_mcp_settings.json", format: Format::Json, at: &["mcpServers"] }),
        slot: Slot::Named,
        entry,
    },
    guidance: Some(Guidance { file: "rules/fastctx.md", head: None, most: None }),
    shell: &[Edit::Add {
        to: To::One(Place { file: "data/settings/global-settings.json", format: Format::Json, at: &["disabledTools"] }),
        item: || json!("run_commands"),
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
