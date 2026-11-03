//! DeepSeek's harness dsh 0.2, a developer preview: its user layer is the
//! patch list `cordis.patch.yml` under DSH_HOME (`~/.dsh` by default),
//! applied to every profile, and an MCP server is a row inserting its MCP
//! client plugin. Its shells come with each profile's presets, so no user
//! setting turns them all off; nor does one raise where a tool's answer is
//! cut (12,500 tokens, the rest kept in a file). A grant under
//! `deepseek-account-platform/default` in `.credentials.yaml` is a
//! signed-in account, which names no email there.
//! See https://github.com/deepseek-ai/deepseek-harness.

use std::path::Path;

use serde_json::{Value, json};

use super::spec::{Agent, Format, Guidance, Home, Mcp, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "deepseek",
    title: "DeepSeek Harness",
    home: Home { env: &[("DSH_HOME", "")], default: ".dsh" },
    marks: &[],
    programs: &[Program::Command("dsh"), Program::Npm("@deepseek-ai/dsh")],
    mcp: Mcp {
        to: To::One(Place { file: "cordis.patch.yml", format: Format::Yaml, at: &[] }),
        slot: Slot::Listed { key: &["insert", "0", "id"], value: ROW },
        entry,
    },
    // Its instructions read together are cut at 64 KiB.
    guidance: Some(Guidance { file: "AGENTS.md", head: None, most: Some(64 * 1024) }),
    shell: &[],
    limits: &[],
    data: None,
    identity,
};

/// The id of Fastctx's row in the patch list.
const ROW: &str = "fastctx-mcp";

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "insert": [{
        "id": ROW,
        "name": "@deepseek-ai/dsh-mcp-client",
        "config": {
            "serverName": "fastctx",
            "transport": "stdio",
            "command": server.command,
            "args": server.args,
            "env": env,
        },
    }] })
}

/// Whether a DeepSeek account is signed in. An API key is another way in,
/// kept elsewhere: without an account it is not known from here.
fn identity(home: &Path) -> Option<fastctx_types::logins::Identity> {
    use fastctx_types::logins::Identity;
    let text = std::fs::read_to_string(home.join(".credentials.yaml")).ok()?;
    let credentials: Value = serde_yaml_ng::from_str(&text).ok()?;
    let record = credentials.get("records")?.get("deepseek-account-platform/default")?;
    let signed = record.get("kind").and_then(Value::as_str) == Some("grant")
        && record.pointer("/payload/token").and_then(Value::as_str).is_some_and(|token| !token.is_empty());
    signed.then(|| Identity { logged_in: true, method: Some("account".into()), ..Identity::default() })
}
