//! Zed 1.28 native agent configuration; external ACP agents use their own settings.
//! Release credentials are kept by the system credential provider.
//! See https://zed.dev/docs/ai/tool-permissions.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Guidance, Home, Mcp, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "zed",
    title: "Zed",
    home: if cfg!(windows) {
        Home { env: &[("APPDATA", "Zed")], default: "AppData/Roaming/Zed" }
    } else if cfg!(target_os = "macos") {
        Home { env: &[], default: ".config/zed" }
    } else {
        Home { env: &[("FLATPAK_XDG_CONFIG_HOME", "zed"), ("XDG_CONFIG_HOME", "zed")], default: ".config/zed" }
    },
    marks: &[],
    programs: &[Program::Command("zed")],
    mcp: Mcp {
        to: To::One(Place { file: "settings.json", format: Format::Jsonc, at: &["context_servers"] }),
        slot: Slot::Named,
        entry,
    },
    guidance: Some(Guidance { file: "AGENTS.md", head: None, most: None }),
    shell: &[Edit::Add {
        to: To::One(Place {
            file: "settings.json",
            format: Format::Jsonc,
            at: &["agent", "tool_permissions", "tools", "terminal", "always_deny"],
        }),
        item: || json!({ "pattern": ".*" }),
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
