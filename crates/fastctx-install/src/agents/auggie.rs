//! Auggie 0.36.0 user configuration. Its native command tool is launch-process.
//! The standard ~/.augment home also contains its global rules; a custom cache alone does not.
//! See https://docs.augmentcode.com/cli/settings.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Home, Mcp, Program, Server};

pub(super) const AGENT: Agent = Agent {
    name: "auggie",
    title: "Auggie",
    home: Home { env: &[], default: ".augment" },
    programs: &[Program::Command("auggie"), Program::Npm("@augmentcode/auggie")],
    mcp: Mcp { file: "settings.json", format: Format::Jsonc, at: &["mcpServers"], entry },
    guidance: Some("rules/fastctx.md"),
    shell: Some(Edit::Add {
        file: "settings.json",
        format: Format::Jsonc,
        at: &["removedTools"],
        item: || json!("launch-process"),
    }),
    limits: &[],
    identity,
};

fn entry(server: &Server<'_>) -> Value {
    // Auggie joins command and args into a shell string before starting stdio.
    let mut env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    let (command, args) = if cfg!(windows) {
        // Auggie 0.36 passes cmd /c through Node's argv quoting (2026-11-02).
        // Expand quoted paths afterward so Node cannot escape their quotes.
        env.insert("FASTCTX_STDIO_COMMAND".into(), json!(quote(server.command)));
        let args: Vec<String> = server
            .args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                let name = format!("FASTCTX_STDIO_ARGUMENT_{index}");
                env.insert(name.clone(), json!(quote(arg)));
                format!("%{name}%")
            })
            .collect();
        ("%FASTCTX_STDIO_COMMAND%".to_owned(), args)
    } else {
        (quote(server.command), server.args.iter().map(|arg| quote(arg)).collect())
    };
    json!({ "type": "stdio", "command": command, "args": args, "env": env })
}

fn quote(value: &str) -> String {
    if cfg!(windows) { format!("\"{value}\"") } else { format!("'{}'", value.replace('\'', "'\\''")) }
}

/// A saved OAuth session names its authentication method, but no account or plan.
fn identity(home: &std::path::Path) -> Option<fastctx_types::logins::Identity> {
    use fastctx_types::logins::Identity;
    let bytes = match std::fs::read(home.join("session.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Identity::default()),
        Err(_) => return None,
    };
    let session: Value = serde_json::from_slice(&bytes).ok()?;
    let present = ["accessToken", "tenantURL"]
        .into_iter()
        .all(|key| session[key].as_str().is_some_and(|value| !value.is_empty()))
        && session["scopes"].is_array();
    Some(if present {
        Identity { logged_in: true, method: Some("oauth".to_owned()), ..Identity::default() }
    } else {
        Identity::default()
    })
}
