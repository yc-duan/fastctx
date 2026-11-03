//! Goose 1.57: its config is `config.yaml` in its config directory
//! (`%APPDATA%/Block/goose/config` on Windows, `~/.config/goose` elsewhere,
//! `config` under GOOSE_PATH_ROOT where that is set), and an MCP server is
//! an entry of `extensions`. Its shell is one of its developer extension's
//! tools, turned off only by listing every other tool it keeps, which would
//! hold back the tools later versions add; so it stays on. A tool's answer
//! past `GOOSE_MAX_TOOL_RESPONSE_SIZE` characters is kept in a file. Goose
//! has no account of its own: providers' keys live in the system keyring.
//! See https://goose-docs.ai/docs/guides/config-files/.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Guidance, Home, Mcp, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "goose",
    title: "Goose",
    home: if cfg!(windows) {
        Home {
            env: &[("GOOSE_PATH_ROOT", "config"), ("APPDATA", "Block/goose/config")],
            default: "AppData/Roaming/Block/goose/config",
        }
    } else {
        Home { env: &[("GOOSE_PATH_ROOT", "config"), ("XDG_CONFIG_HOME", "goose")], default: ".config/goose" }
    },
    marks: &[],
    programs: &[Program::Command("goose")],
    mcp: Mcp {
        to: To::One(Place { file: "config.yaml", format: Format::Yaml, at: &["extensions"] }),
        slot: Slot::Named,
        entry,
    },
    guidance: Some(Guidance { file: "AGENTS.md", head: None, most: None }),
    shell: &[],
    limits: &[Edit::Raise {
        to: To::One(Place { file: "config.yaml", format: Format::Yaml, at: &["GOOSE_MAX_TOOL_RESPONSE_SIZE"] }),
        value: || json!(1_000_000),
    }],
    data: None,
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({
        "name": "fastctx",
        "type": "stdio",
        "enabled": true,
        "cmd": server.command,
        "args": server.args,
        "envs": env,
        "env_keys": [],
        "timeout": 300,
    })
}
