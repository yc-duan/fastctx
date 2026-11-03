//! Who is logged in to a Codex home, as its files say
//! (`world-agent-logins.md`, 认出是谁): `auth.json` tells how it signs in,
//! and a ChatGPT login's identity token names its account and plan. Only
//! that token's claims are read; no token is kept, logged or sent. Nothing
//! is started and nothing goes over the network.

use std::path::Path;

use base64::Engine as _;
use fastctx_types::agents::Fact;
use fastctx_types::logins::Identity;
use serde_json::Value;

/// Who is logged in to the Codex home `home`, named as its app-server's
/// `account/read` names it: how (`chatgpt`, `apiKey`, or as `auth.json`
/// writes another way), and for a ChatGPT login the account's email and
/// plan. With no `auth.json` no one is, unless the provider configured
/// needs no OpenAI login, or the sign-in is kept where no file shows it.
pub fn identity(home: &Path) -> Fact<Identity> {
    let path = home.join("auth.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let config = Config::read(home);
            return match config.store.as_deref() {
                Some("keyring" | "auto" | "ephemeral") => {
                    Fact::unavailable("its sign-in is kept in the system's keyring")
                }
                _ if !config.needs_openai_login() => Fact::known(Identity { logged_in: true, ..Identity::default() }),
                _ => Fact::known(Identity::default()),
            };
        }
        Err(error) => return Fact::failed(format!("reading {}: {error}", path.display())),
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(auth) => Fact::known(of(&auth)),
        Err(error) => Fact::failed(format!("reading {}: {error}", path.display())),
    }
}

/// The identity `auth.json` holds, its way of signing in decided as Codex
/// decides it (`AuthDotJson::resolved_mode`).
fn of(auth: &Value) -> Identity {
    let mode = auth["auth_mode"].as_str().map(str::to_owned).unwrap_or_else(|| {
        // Written before Codex kept how it signs in: a key is an API key
        // login even beside tokens.
        if auth["OPENAI_API_KEY"].is_null() { "chatgpt".into() } else { "apikey".into() }
    });
    if mode.eq_ignore_ascii_case("apikey") {
        let key = auth["OPENAI_API_KEY"].as_str().is_some_and(|key| !key.is_empty());
        return Identity { logged_in: key, method: key.then(|| "apiKey".to_owned()), ..Identity::default() };
    }
    if !mode.starts_with("chatgpt") {
        // A personal access token or a cloud provider's keys: signed in, by
        // no account the file names.
        return Identity { logged_in: true, method: Some(mode), ..Identity::default() };
    }
    if !auth["tokens"].is_object() {
        return Identity::default();
    }
    let claims = auth["tokens"]["id_token"].as_str().and_then(claims);
    let text = |value: &Value| value.as_str().filter(|text| !text.is_empty()).map(str::to_owned);
    let email = claims
        .as_ref()
        .and_then(|claims| text(&claims["email"]).or_else(|| text(&claims["https://api.openai.com/profile"]["email"])));
    let plan = claims.as_ref().and_then(|claims| text(&claims["https://api.openai.com/auth"]["chatgpt_plan_type"]));
    Identity { logged_in: true, method: Some("chatgpt".to_owned()), email, organization: None, plan }
}

/// The claims of a JSON Web Token, its signature unchecked: they only name
/// whom the login was given to.
fn claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let (Some(_), Some(payload), Some(_), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// What `config.toml` says of signing in: where credentials are kept, and
/// which provider the models come from, the profile chosen there first.
struct Config {
    store: Option<String>,
    provider: Option<String>,
    requires_openai_auth: Option<bool>,
}

impl Config {
    fn read(home: &Path) -> Self {
        let table: toml::Table = std::fs::read_to_string(home.join("config.toml"))
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
        let text = |value: Option<&toml::Value>| value.and_then(toml::Value::as_str).map(str::to_owned);
        let profile = text(table.get("profile"))
            .and_then(|name| table.get("profiles").and_then(|profiles| profiles.get(&name)).cloned());
        let provider = text(profile.as_ref().and_then(|profile| profile.get("model_provider")))
            .or_else(|| text(table.get("model_provider")));
        let requires_openai_auth = provider
            .as_ref()
            .and_then(|provider| table.get("model_providers")?.get(provider)?.get("requires_openai_auth")?.as_bool());
        Self { store: text(table.get("cli_auth_credentials_store")), provider, requires_openai_auth }
    }

    /// Whether the provider configured needs an OpenAI login: OpenAI's own
    /// does, one of the user's own only when it says so.
    fn needs_openai_login(&self) -> bool {
        match self.provider.as_deref() {
            None | Some("openai") => true,
            Some(_) => self.requires_openai_auth.unwrap_or(false),
        }
    }
}
