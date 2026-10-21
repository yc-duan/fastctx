//! Per-session settings for a login assisting someone other than its owner.

use std::ffi::OsString;
use std::path::Path;

use serde_json::{Value, json};

/// Keeps authentication from the login's settings when its instruction and
/// extension sources are disabled. Credentials stored by Claude itself remain
/// in the original login; no setting is written back to it.
pub fn configure(settings: &mut Value, home: &Path, environment: &[(OsString, OsString)]) -> Result<(), String> {
    let path = home.join("settings.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("reading the Claude login settings: {error}")),
    };
    let login: Value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).map_err(|error| {
            format!("invalid Claude login settings at line {}, column {}", error.line(), error.column())
        })?
    };
    if let Some(configured) = login.get("env").and_then(Value::as_object) {
        for (name, value) in configured {
            if AUTH_ENV.contains(&name.as_str())
                && !environment.iter().any(|(key, value)| key.eq_ignore_ascii_case(name) && !value.is_empty())
            {
                settings["env"][name] = value.clone();
            }
        }
    }
    for name in ["apiKeyHelper", "awsAuthRefresh", "awsCredentialExport", "gcpAuthRefresh", "proxyAuthHelper"] {
        if let Some(value) = login.get(name) {
            settings[name] = value.clone();
        }
    }
    settings["env"]["FASTCTX_ASSISTED"] = "1".into();
    settings["env"]["CLAUDE_CODE_DISABLE_CLAUDE_MDS"] = "1".into();
    settings["env"]["CLAUDE_CODE_DISABLE_AUTO_MEMORY"] = "1".into();
    settings["env"]["CLAUDE_CODE_DISABLE_POLICY_SKILLS"] = "1".into();
    settings["env"]["CLAUDE_CODE_PLUGIN_DIRS"] = "".into();
    settings["syncClaudeAiPlugins"] = false.into();
    settings["autoMemoryEnabled"] = false.into();
    let denied = settings["permissions"]["deny"].as_array_mut().ok_or("the session has no tool policy")?;
    for tool in [
        "ListAgents",
        "SendMessage",
        "EnterWorktree",
        "ExitWorktree",
        "Workflow",
        "Skill",
        "CronCreate",
        "CronDelete",
        "CronList",
    ] {
        denied.push(tool.into());
    }
    Ok(())
}

const AUTH_ENV: &[&str] = &[
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
    "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_USE_GATEWAY",
    "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
    "CLAUDE_CODE_SKIP_VERTEX_AUTH",
    "CLAUDE_CODE_SKIP_FOUNDRY_AUTH",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_BEARER_TOKEN_BEDROCK",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_VERTEX_BASE_URL",
    "ANTHROPIC_AWS_BASE_URL",
    "ANTHROPIC_GOOGLE_CLOUD_BASE_URL",
    "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
    "ANTHROPIC_AWS_API_KEY",
    "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_RESOURCE",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "CLAUDE_CODE_CLIENT_CERT",
    "CLAUDE_CODE_CLIENT_KEY",
    "CLAUDE_CODE_CLIENT_KEY_PASSPHRASE",
];

/// Conversation operations whose implementations leave the login unchanged.
pub fn command_allowed(name: &str) -> bool {
    matches!(name, "rename" | "model" | "goal" | "compact" | "btw")
}

pub const COMMAND_REFUSED: &str = "This command is unavailable in an assisted conversation because it can access or change the host login or computer.";
