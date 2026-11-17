//! Qwen Code 0.26.6: settings and credentials follow QWEN_HOME.
//! The Bash deny rule removes its shell, while MCP keeps its own permission names.
//! MCP still has a fixed 500,000-character producer limit.
//! See https://github.com/QwenLM/qwen-code/tree/v0.26.6/packages/core/src.

use serde_json::{Value, json};

use super::spec::{Agent, Edit, Format, Guidance, Home, Mcp, Place, Program, Server, Slot, To};

pub(super) const AGENT: Agent = Agent {
    name: "qwen",
    title: "Qwen Code",
    home: Home { env: &[("QWEN_HOME", "")], default: ".qwen" },
    marks: &[],
    programs: &[Program::Command("qwen"), Program::Npm("@qwen-code/qwen-code")],
    mcp: Mcp {
        to: To::One(Place { file: "settings.json", format: Format::Jsonc, at: &["mcpServers"] }),
        slot: Slot::Named,
        entry,
    },
    guidance: Some(Guidance { file: "QWEN.md", head: None, most: None }),
    shell: &[Edit::Add {
        to: To::One(Place { file: "settings.json", format: Format::Jsonc, at: &["permissions", "deny"] }),
        item: || json!("Bash"),
    }],
    limits: &[
        Edit::Raise {
            to: To::One(Place {
                file: "settings.json",
                format: Format::Jsonc,
                at: &["tools", "truncateToolOutputThreshold"],
            }),
            value: || json!(-1),
        },
        Edit::Raise {
            to: To::One(Place {
                file: "settings.json",
                format: Format::Jsonc,
                at: &["tools", "truncateToolOutputLines"],
            }),
            value: || json!(-1),
        },
        Edit::Raise {
            to: To::One(Place {
                file: "settings.json",
                format: Format::Jsonc,
                at: &["tools", "toolOutputBatchBudget"],
            }),
            value: || json!(-1),
        },
    ],
    data: None,
    identity,
};

fn entry(server: &Server<'_>) -> Value {
    let env: serde_json::Map<String, Value> =
        server.env.iter().map(|(name, value)| (name.clone(), json!(value))).collect();
    json!({ "command": server.command, "args": server.args, "env": env })
}

/// Reads the selected file-backed login without treating a provider choice as a credential.
fn identity(home: &std::path::Path) -> Option<fastctx_types::logins::Identity> {
    use fastctx_types::logins::Identity;
    let settings = read(home, "settings.json")?;
    let method = settings.pointer("/security/auth/selectedType").and_then(Value::as_str);

    // Without a chosen way it may still log in from the environment.
    let method = method?;
    let present = if method == "qwen-oauth" {
        let credentials = read(home, "oauth_creds.json")?;
        credentials.get("access_token").and_then(Value::as_str).is_some_and(|value| !value.is_empty())
    } else if matches!(method, "openai" | "openai-responses" | "gemini" | "anthropic") {
        if settings.get("modelProviders").is_some() {
            return None;
        }
        settings.pointer("/security/auth/apiKey").and_then(Value::as_str).is_some_and(|value| !value.is_empty())
    } else {
        return None;
    };
    Some(if present {
        Identity { logged_in: true, method: Some(method.to_owned()), ..Identity::default() }
    } else {
        Identity::default()
    })
}

fn read(home: &std::path::Path, name: &str) -> Option<Value> {
    let text = match fastctx_platform::disk::read_to_string(home.join(name)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Value::Null),
        Err(_) => return None,
    };
    jsonc_parser::parse_to_serde_value(&text, &Default::default()).ok()
}
