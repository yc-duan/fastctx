//! Assisted threads have a separate instruction, extension and session home.
//! The original app-server remains the login authority. Access tokens or API
//! keys cross its official local RPC interface in memory, never refresh tokens.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use fastctx_platform::{Permissions, ensure_private_dir, publish};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{CodexHost, HostEvent, launch::Launch, server::AppServer};

pub(super) struct Assisted {
    pub owner: Arc<CodexHost>,
    pub home: PathBuf,
}

impl CodexHost {
    /// Creates an isolated host using the original login through local RPC.
    pub fn new_assisted(
        owner: Arc<Self>,
        workdir: PathBuf,
    ) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<HostEvent>), String> {
        let home = workdir.join("config");
        let user = workdir.join("user");
        for path in [&workdir, &home, &user] {
            ensure_private_dir(path).map_err(|error| format!("creating the assisted Codex home: {error}"))?;
        }
        let mut env = owner.env.clone();
        env.retain(|(key, _)| {
            !["CODEX_HOME", "HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "APPDATA", "LOCALAPPDATA"]
                .iter()
                .any(|name| key.eq_ignore_ascii_case(name))
        });
        for (name, path) in [
            ("CODEX_HOME", home.clone()),
            ("HOME", user.clone()),
            ("USERPROFILE", user.clone()),
            ("XDG_CONFIG_HOME", user.join(".config")),
            ("XDG_DATA_HOME", user.join(".local/share")),
            ("APPDATA", user.join("AppData/Roaming")),
            ("LOCALAPPDATA", user.join("AppData/Local")),
        ] {
            env.push((OsString::from(name), path.into_os_string()));
        }
        let (mut host, events) =
            Self::new(owner.codex_home.clone(), owner.program_home.clone(), workdir, env, owner.log.clone());
        Arc::get_mut(&mut host).expect("new host has one owner").assisted = Some(Assisted { owner, home });
        Ok((host, events))
    }

    /// Where this host stores threads, which differs from the login's home
    /// for assisted conversations.
    pub fn session_home(&self) -> &Path {
        self.assisted.as_ref().map_or(&self.codex_home, |assisted| &assisted.home)
    }

    /// Assisted conversations stand outside the owner's project hierarchy.
    pub fn assisted_cwd(&self) -> Option<&Path> {
        self.assisted.as_ref().map(|_| self.workdir.as_path())
    }
}

impl Assisted {
    pub(super) async fn configure(&self, server: &AppServer, launch: &mut Launch) -> Result<(), String> {
        let read = server.request("config/read", json!({"includeLayers":false})).await?;
        let inherited = read["config"].as_object().ok_or("Codex did not provide its effective login configuration")?;
        let mut config = json!({});
        // Only model transport and selection survive; paths to instructions,
        // extensions, hooks, agents and memory deliberately do not.
        for name in [
            "model_provider",
            "model_providers",
            "model",
            "model_reasoning_effort",
            "model_reasoning_summary",
            "model_context_window",
            "model_auto_compact_token_limit",
            "model_verbosity",
            "chatgpt_base_url",
            "forced_chatgpt_workspace_id",
            "forced_login_method",
            "web_search",
            "service_tier",
        ] {
            if let Some(value) = inherited.get(name).filter(|value| !value.is_null()) {
                config[name] = value.clone();
            }
        }
        configure_thread(&mut config);
        config["cli_auth_credentials_store"] = "ephemeral".into();
        without_nulls(&mut config);
        let table: toml::Table = serde_json::from_value(config).map_err(|_| "invalid assisted Codex configuration")?;
        let bytes = toml::to_string(&table).map_err(|_| "serializing assisted Codex configuration")?;
        publish(&self.home.join("config.toml"), bytes.as_bytes(), Permissions::Private)
            .map_err(|error| format!("writing assisted Codex configuration: {error}"))?;
        launch.args = vec!["app-server".into()];
        launch.assisted_config = Some(table);
        Ok(())
    }

    pub(super) async fn login(&self, owner: &AppServer, server: &AppServer) -> Result<(), String> {
        let status = owner.request("getAuthStatus", json!({"includeToken":true,"refreshToken":false})).await?;
        if status["requiresOpenaiAuth"] == false {
            return Ok(());
        }
        let token = status["authToken"]
            .as_str()
            .filter(|token| !token.is_empty())
            .ok_or("the Codex login did not provide credentials for the assisted host")?;
        let request = match status["authMethod"].as_str() {
            Some("apikey") => json!({"type":"apiKey","apiKey":token}),
            Some("chatgpt" | "chatgptAuthTokens") => {
                let mut tokens = chatgpt_tokens(owner, token).await?;
                tokens["type"] = "chatgptAuthTokens".into();
                tokens
            }
            _ => return Err("this Codex authentication method cannot be used in an isolated assisted host".into()),
        };
        server
            .request("account/login/start", request)
            .await
            .map_err(|_| "Codex refused the assisted host's login".to_string())?;
        Ok(())
    }

    pub(super) async fn refresh(&self, previous: Option<&str>) -> Result<Value, String> {
        let owner = self.owner.server().await?;
        let status = owner.request("getAuthStatus", json!({"includeToken":true,"refreshToken":true})).await?;
        let token = status["authToken"]
            .as_str()
            .filter(|token| !token.is_empty())
            .ok_or("the original Codex login could not refresh its credentials")?;
        let tokens = chatgpt_tokens(&owner, token).await?;
        if previous.is_some_and(|previous| tokens["chatgptAccountId"].as_str() != Some(previous)) {
            return Err("the Codex login changed accounts during this assisted conversation".into());
        }
        Ok(tokens)
    }
}

fn without_nulls(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|_, value| !value.is_null());
            for value in fields.values_mut() {
                without_nulls(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                without_nulls(value);
            }
        }
        _ => {}
    }
}

async fn chatgpt_tokens(owner: &AppServer, token: &str) -> Result<Value, String> {
    let payload = token.split('.').nth(1).ok_or("Codex provided an access token without account claims")?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| "Codex provided invalid access-token claims")?;
    let claims: Value = serde_json::from_slice(&decoded).map_err(|_| "Codex provided invalid access-token claims")?;
    let auth = &claims["https://api.openai.com/auth"];
    let account = auth["chatgpt_account_id"].as_str().ok_or("Codex provided no account identity for assistance")?;
    let config = owner.request("config/read", json!({"includeLayers":false})).await?;
    if config["config"]["forced_chatgpt_workspace_id"].as_str().is_some_and(|forced| forced != account) {
        return Err("the Codex access token does not identify the configured workspace".into());
    }
    Ok(json!({"accessToken":token,"chatgptAccountId":account,"chatgptPlanType":auth["chatgpt_plan_type"]}))
}

/// Applied after account preferences, at both birth and cold resume.
pub fn configure_thread(config: &mut Value) {
    for name in ["plugins", "remote_plugin", "recommended_plugins", "memories", "hooks", "apps"] {
        config["features"][name] = false.into();
    }
    config["project_doc_max_bytes"] = 0.into();
    config["skills"] = json!({"include_instructions":false,"bundled":{"enabled":false}});
    config["memories"] = json!({"generate_memories":false,"use_memories":false});
    config["orchestrator"] = json!({"mcp":{"enabled":false},"skills":{"enabled":false}});
}

pub fn command_allowed(name: &str) -> bool {
    matches!(name, "compact" | "model" | "rename" | "fork" | "goal" | "tier")
}

pub const COMMAND_REFUSED: &str = "This command is unavailable in an assisted conversation because it can access or change the host login or computer.";
