//! Claude Desktop 1.4758 user-data configuration, separate from Claude Code.
//! No stable local instruction file or user shell switch was established for Desktop Chat.
//! Its command detection deliberately does not match Claude Code. On Windows its Store
//! package writes what it keeps under AppData into the package's own folder, and reads its
//! configuration there first once it wrote one; the package's manifest does not exclude it.

use serde_json::{Value, json};

use super::spec::{Agent, Format, Home, Mcp, Pick, Place, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "claude-desktop",
    title: "Claude Desktop",
    home: if cfg!(windows) {
        Home { env: &[("APPDATA", "Claude")], default: "AppData/Roaming/Claude" }
    } else if cfg!(target_os = "macos") {
        Home { env: &[], default: "Library/Application Support/Claude" }
    } else {
        Home { env: &[("XDG_CONFIG_HOME", "Claude")], default: ".config/Claude" }
    },
    marks: if cfg!(windows) { &[".", PACKAGED] } else { &[] },
    programs: &[],
    mcp: Mcp {
        to: if cfg!(windows) {
            To::Among(CONFIGS, Pick::Present { made: 1 })
        } else {
            To::One(Place { file: "claude_desktop_config.json", format: Format::Json, at: &["mcpServers"] })
        },
        slot: Slot::Named,
        entry,
    },
    guidance: None,
    shell: &[],
    limits: &[],
    data: None,
    identity: |_| None,
};

/// What the Store package keeps for `%APPDATA%/Claude`, beside the home.
const PACKAGED: &str = "../../Local/Packages/Claude_pzs8sxrjxfjjc/LocalCache/Roaming/Claude";

/// Read in the package's folder first; where neither is there, the plain
/// one is made, which the package reads through.
const CONFIGS: &[Place] = &[
    Place {
        file: "../../Local/Packages/Claude_pzs8sxrjxfjjc/LocalCache/Roaming/Claude/claude_desktop_config.json",
        format: Format::Json,
        at: &["mcpServers"],
    },
    Place { file: "claude_desktop_config.json", format: Format::Json, at: &["mcpServers"] },
];

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}
