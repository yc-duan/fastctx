//! OpenCode 1.19.5 loads opencode.jsonc after opencode.json.
//! Its credential store is in a separate XDG data directory.
//! See https://github.com/anomalyco/opencode/tree/v1.19.5/packages/opencode/src.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Home, Mcp, Program, Server};

pub(super) const AGENT: Agent = Agent {
    name: "opencode",
    title: "OpenCode",
    home: Home { env: &[("OPENCODE_CONFIG_DIR", ""), ("XDG_CONFIG_HOME", "opencode")], default: ".config/opencode" },
    programs: &[Program::Command("opencode"), Program::Npm("opencode-ai")],
    mcp: Mcp { file: "opencode.jsonc", format: Format::Jsonc, at: &["mcp"], entry },
    guidance: Some("AGENTS.md"),
    shell: Some(Edit::Set {
        file: "opencode.jsonc",
        format: Format::Jsonc,
        at: &["permission", "bash"],
        value: || json!("deny"),
    }),
    limits: &[
        Edit::Set {
            file: "opencode.jsonc",
            format: Format::Jsonc,
            at: &["tool_output", "max_bytes"],
            value: || json!(1_048_576),
        },
        Edit::Set {
            file: "opencode.jsonc",
            format: Format::Jsonc,
            at: &["tool_output", "max_lines"],
            value: || json!(100_000),
        },
    ],
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    let command: Vec<&str> = std::iter::once(server.command).chain(server.args.iter().map(String::as_str)).collect();
    let environment: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "type": "local", "command": command, "environment": environment })
}
