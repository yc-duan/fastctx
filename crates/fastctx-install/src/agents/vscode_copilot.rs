//! VS Code 1.117 default-profile native MCP configuration.
//! Copilot harness instructions and authentication belong to its separate COPILOT_HOME.
//! See https://code.visualstudio.com/docs/agents/reference/mcp-configuration.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "vscode-copilot",
    title: "Copilot (VS Code)",
    home: if cfg!(windows) {
        Home {
            env: &[("VSCODE_PORTABLE", "user-data/User"), ("VSCODE_APPDATA", "Code/User"), ("APPDATA", "Code/User")],
            default: "AppData/Roaming/Code/User",
        }
    } else if cfg!(target_os = "macos") {
        Home {
            env: &[("VSCODE_PORTABLE", "user-data/User"), ("VSCODE_APPDATA", "Code/User")],
            default: "Library/Application Support/Code/User",
        }
    } else {
        Home {
            env: &[
                ("VSCODE_PORTABLE", "user-data/User"),
                ("VSCODE_APPDATA", "Code/User"),
                ("XDG_CONFIG_HOME", "Code/User"),
            ],
            default: ".config/Code/User",
        }
    },
    marks: &[],
    programs: &[Program::Command("code")],
    mcp: Mcp {
        to: To::One(Place { file: "mcp.json", format: Format::Jsonc, at: &["servers"] }),
        slot: Slot::Named,
        entry,
    },
    guidance: None,
    shell: &[],
    limits: &[],
    data: None,
    identity: |_| None,
};

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env, "type": "stdio" })
}
