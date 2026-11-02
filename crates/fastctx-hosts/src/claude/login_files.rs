//! Who is logged in to a Claude Code configuration directory, as its files
//! say (`world-agent-logins.md`, 认出是谁): how it signs in, decided as
//! `claude auth status` decides it, and for a claude.ai login its account,
//! organization and plan. No credential is kept, logged or sent; nothing is
//! started and nothing goes over the network.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use fastctx_types::agents::Fact;
use fastctx_types::logins::Identity;
use serde_json::Value;

/// Who is logged in to the configuration directory `configured` names, the
/// value of `CLAUDE_CONFIG_DIR` its sessions start with (`None` is unset),
/// its credentials where `secure`, the user's
/// `CLAUDE_SECURESTORAGE_CONFIG_DIR`, puts them: named as `claude auth
/// status` names it (`claude.ai`, `oauth_token`, `api_key`, …). A token or
/// key the user's own environment sets is not seen; Claude Code's sessions
/// would use it before any of these.
pub fn identity(configured: Option<&OsStr>, secure: Option<&OsStr>) -> Fact<Identity> {
    let home = match user_home() {
        Ok(home) => home,
        Err(error) => return Fact::failed(format!("finding the home directory: {error}")),
    };
    let config =
        configured.filter(|value| !value.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".claude"));
    let settings = read(&config.join("settings.json"));
    let env = |name: &str| settings["env"][name].as_str().is_some_and(|value| !value.is_empty());
    // Claude Code's own test for a variable that turns something on.
    let on = |name: &str| {
        settings["env"][name]
            .as_str()
            .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
    };
    // As `claude auth status` orders them: another provider's models, a
    // token in the environment Claude Code is given, a helper that makes
    // keys, the claude.ai login, then a key.
    if ["CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"].into_iter().any(on) {
        return Fact::known(signed("third_party"));
    }
    if env("ANTHROPIC_AUTH_TOKEN") || env("CLAUDE_CODE_OAUTH_TOKEN") {
        return Fact::known(signed("oauth_token"));
    }
    if settings["apiKeyHelper"].as_str().is_some_and(|helper| !helper.trim().is_empty()) {
        return Fact::known(signed("api_key_helper"));
    }
    let state = match crate::claude::trust::state_file(configured) {
        Ok(path) => read(&path),
        Err(error) => return Fact::failed(format!("finding Claude Code's state file: {error}")),
    };
    let credentials_dir = match secure {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        Some(_) => home.join(".claude"),
        None => config,
    };
    let credentials = read(&credentials_dir.join(".credentials.json"));
    let oauth = &credentials["claudeAiOauth"];
    let account = &state["oauthAccount"];
    let text = |value: &Value| value.as_str().filter(|text| !text.is_empty()).map(str::to_owned);
    let inference =
        oauth["scopes"].as_array().is_some_and(|scopes| scopes.iter().any(|scope| scope == "user:inference"));
    // Credentials the system's keyring keeps leave none in the file, and the
    // account stays in the state file until logging out takes both away.
    let kept_elsewhere = oauth.is_null() && account.is_object() && text(&state["primaryApiKey"]).is_none();
    if (inference && text(&oauth["accessToken"]).is_some()) || kept_elsewhere {
        return Fact::known(Identity {
            logged_in: true,
            method: Some("claude.ai".to_owned()),
            email: text(&account["emailAddress"]),
            organization: text(&account["organizationName"]),
            plan: text(&oauth["subscriptionType"]),
        });
    }
    if env("ANTHROPIC_API_KEY") {
        return Fact::known(signed("api_key"));
    }
    if text(&state["primaryApiKey"]).is_some() {
        return Fact::known(Identity {
            logged_in: true,
            method: Some("api_key".to_owned()),
            email: text(&account["emailAddress"]),
            organization: text(&account["organizationName"]),
            plan: None,
        });
    }
    Fact::known(Identity::default())
}

fn signed(method: &str) -> Identity {
    Identity { logged_in: true, method: Some(method.to_owned()), ..Identity::default() }
}

/// A JSON file's value; null where it is not there or not JSON.
fn read(path: &Path) -> Value {
    std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or(Value::Null)
}

/// The user's home as Claude Code finds it.
fn user_home() -> std::io::Result<PathBuf> {
    #[cfg(windows)]
    if let Some(profile) = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(profile));
    }
    fastctx_platform::user_home()
}
