//! Amp: its user settings are `settings.json` in its config directory,
//! read alone where it is there, else `settings.jsonc`; the two are never
//! merged. MCP servers are under the literal key `amp.mcpServers`, and its
//! built-in shells are turned off by name in `amp.tools.disable`. Each MCP
//! text block is cut at 262,144 bytes, which no setting raises. Who is
//! logged in is in `accounts.json` in its data directory.
//! See https://ampcode.com/docs/cli/settings.

use std::path::Path;

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Guidance, Home, Mcp, Pick, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "amp",
    title: "Amp",
    home: Home { env: &[("XDG_CONFIG_HOME", "amp")], default: ".config/amp" },
    marks: &[],
    programs: &[Program::Command("amp"), Program::Npm("@ampcode/cli")],
    mcp: Mcp { to: To::Among(SERVERS, Pick::Present { made: 0 }), slot: Slot::Named, entry },
    guidance: Some(Guidance { file: "AGENTS.md", head: None, most: None }),
    shell: &[Edit::Add {
        to: To::Among(DISABLED, Pick::Present { made: 0 }),
        item: || json!("builtin:{Bash,shell_command,async_shell_command}"),
    }],
    limits: &[],
    data: Some(Home { env: &[("XDG_DATA_HOME", "amp")], default: ".local/share/amp" }),
    identity,
};

const SERVERS: &[Place] = &[
    Place { file: "settings.json", format: Format::Jsonc, at: &["amp.mcpServers"] },
    Place { file: "settings.jsonc", format: Format::Jsonc, at: &["amp.mcpServers"] },
];

const DISABLED: &[Place] = &[
    Place { file: "settings.json", format: Format::Jsonc, at: &["amp.tools.disable"] },
    Place { file: "settings.jsonc", format: Format::Jsonc, at: &["amp.tools.disable"] },
];

/// The service it logs in to unless told another, as its accounts name it.
const SERVICE: &str = "https://ampcode.com/";

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}

/// The account saved as the one in use for its service. With none saved it
/// may still log in from an access key, in the environment or kept by an
/// earlier version: that is not known from here.
fn identity(data: &Path) -> Option<fastctx_types::logins::Identity> {
    use fastctx_types::logins::Identity;
    let accounts: Value =
        serde_json::from_slice(&fastctx_platform::disk::read(data.join("accounts.json")).ok()?).ok()?;
    let active = accounts.pointer("/active")?.get(SERVICE)?.as_str()?;
    let account = accounts.get("accounts")?.as_array()?.iter().find(|account| {
        account.get("serverURL").and_then(Value::as_str) == Some(SERVICE)
            && account.get("userID").and_then(Value::as_str) == Some(active)
    })?;
    // One taken over from an access key names the key, not a person.
    if account.get("legacy").is_some_and(|legacy| !legacy.is_null()) {
        return Some(Identity { logged_in: true, method: Some("apikey".into()), ..Identity::default() });
    }
    let email = account.get("email").and_then(Value::as_str).filter(|email| email.contains('@'))?;
    Some(Identity { logged_in: true, email: Some(email.to_owned()), ..Identity::default() })
}
