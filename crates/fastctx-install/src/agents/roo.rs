//! Roo Code 3.53.0 in the default VS Code profile. Other editors, profiles and
//! customStoragePath need their actual storage root supplied by the caller.
//! Its global rules and SecretStorage credentials are outside this root.
//! See https://github.com/RooCodeInc/Roo-Code/tree/v3.53.0.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Server};

pub(super) const AGENT: Agent = Agent {
    name: "roo",
    title: "Roo Code (VS Code)",
    home: if cfg!(windows) {
        Home {
            env: &[("APPDATA", "Code/User/globalStorage/rooveterinaryinc.roo-cline")],
            default: "AppData/Roaming/Code/User/globalStorage/rooveterinaryinc.roo-cline",
        }
    } else if cfg!(target_os = "macos") {
        Home { env: &[], default: "Library/Application Support/Code/User/globalStorage/rooveterinaryinc.roo-cline" }
    } else {
        Home {
            env: &[("XDG_CONFIG_HOME", "Code/User/globalStorage/rooveterinaryinc.roo-cline")],
            default: ".config/Code/User/globalStorage/rooveterinaryinc.roo-cline",
        }
    },
    programs: &[],
    mcp: Mcp { file: "settings/mcp_settings.json", format: Format::Json, at: &["mcpServers"], entry },
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
