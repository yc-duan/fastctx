//! Gemini CLI (github.com/google-gemini/gemini-cli). Its user-level
//! settings are `~/.gemini/settings.json`, where MCP servers are kept under
//! `mcpServers`; its global instructions are `~/.gemini/GEMINI.md`. Its
//! shell, the built-in `run_shell_command`, is denied by a policy file of
//! Fastctx's own among the user's policies, `~/.gemini/policies/*.toml`,
//! whose rules outrank the defaults. `GEMINI_CLI_HOME` moves the user's
//! home it keeps `.gemini` under.

use std::path::Path;

use fastctx_types::logins::Identity;
use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Home, Mcp, Program, Server};

pub(super) const AGENT: Agent = Agent {
    name: "gemini",
    title: "Gemini CLI",
    home: Home { env: &[("GEMINI_CLI_HOME", ".gemini")], default: ".gemini" },
    programs: &[Program::Command("gemini"), Program::Npm("@google/gemini-cli")],
    mcp: Mcp { file: "settings.json", format: Format::Json, at: &["mcpServers"], entry },
    guidance: Some("GEMINI.md"),
    shell: Some(Edit::Own { file: "policies/fastctx.toml", body: POLICY }),
    limits: &[],
    identity,
};

const POLICY: &str = "# Fastctx's: commands go through its run tool. Taking Fastctx out deletes this file.\n[[rule]]\ntoolName = \"run_shell_command\"\ndecision = \"deny\"\npriority = 100\n";

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}

/// The account `google_accounts.json` names active, and how the settings
/// say Gemini CLI signs in.
fn identity(home: &Path) -> Option<Identity> {
    let read = |name: &str| -> Option<Value> { serde_json::from_slice(&std::fs::read(home.join(name)).ok()?).ok() };
    let settings = read("settings.json");
    let method = settings
        .as_ref()
        .and_then(|settings| {
            settings.pointer("/security/auth/selectedType").or_else(|| settings.get("selectedAuthType"))
        })
        .and_then(Value::as_str)
        .map(str::to_owned);
    let email = read("google_accounts.json")
        .and_then(|accounts| accounts.get("active").and_then(Value::as_str).map(str::to_owned));
    if method.is_none() && email.is_none() {
        return None;
    }
    let logged_in = email.is_some() || matches!(method.as_deref(), Some("gemini-api-key" | "vertex-ai"));
    Some(Identity { logged_in, method, email, organization: None, plan: None })
}
