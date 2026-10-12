//! Asking a Codex login who is logged in there, its limits and its models,
//! through its own app-server, and leaving nothing running for the asking:
//! the app-server already serving the login is asked when there is one;
//! otherwise one is started for the questions and stopped once answered.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use fastctx_types::agents::{AgentModel, Fact};
use fastctx_types::logins::Identity;
use jiff::Timestamp;
use serde_json::{Value, json};

use super::launch::Launch;
use super::server::{AppServer, Inbound};
use super::{CodexHost, Log};

/// What a login's app-server said of it.
#[derive(Debug)]
pub struct Said {
    /// The program's version, from its answer to `initialize`; none when
    /// the app-server asked was already running.
    pub version: Option<String>,
    pub who: Fact<Identity>,
    /// The answer of `account/rateLimits/read`, and when it came; none
    /// when no one is logged in, or the limits were not asked for.
    pub limits: Option<Result<(Timestamp, Value), String>>,
    pub models: Fact<Vec<AgentModel>>,
}

impl CodexHost {
    /// Asks this host's app-server when it is running now, its usage limits
    /// too with `limits`; `None` when it is not, and then nothing is started.
    pub async fn ask_running(self: &Arc<Self>, limits: bool) -> Option<Said> {
        let server = self.lock().server.as_ref().map(|(server, _)| server.clone())?;
        let said = ask(&server, None, limits).await;
        if let Some(Ok((at, answer))) = &said.limits {
            self.limits_heard(*at, super::account::LimitsHow::Read(answer.clone()));
        }
        Some(said)
    }

    /// The program this host starts, as it would start it now.
    pub fn launch(&self) -> Result<Option<Launch>, String> {
        let path = self.env.iter().find(|(name, _)| name.eq_ignore_ascii_case("PATH")).map(|(_, value)| value);
        super::launch::discover_for(&self.codex_home, self.program_home.as_deref(), path)
    }
}

/// Starts `launch` for the Codex home `home`, asks it, its usage limits
/// too with `limits`, and stops it.
pub async fn ask_once(
    launch: &Launch,
    home: &Path,
    cwd: &Path,
    env: &[(OsString, OsString)],
    log: Log,
    limits: bool,
) -> Result<Said, String> {
    let mut env = env.to_vec();
    env.retain(|(name, _)| !name.eq_ignore_ascii_case("CODEX_HOME"));
    env.push(("CODEX_HOME".into(), home.as_os_str().to_owned()));
    let (server, mut inbound) = AppServer::spawn(&launch.binary, &launch.args, cwd, &env, log)
        .map_err(|error| format!("starting {}: {error}", launch.binary.display()))?;
    // Answers come back through this; the app-server asks nothing of a
    // client that starts no thread.
    let answering = server.clone();
    let pump = tokio::spawn(async move {
        while let Some(message) = inbound.recv().await {
            match message {
                Inbound::Response { waiter, answer } => {
                    let _ = waiter.send(answer);
                }
                Inbound::Request { id, method, .. } => {
                    let _ =
                        answering.refuse(id, -32601, &format!("{method} is not answered while identifying a login"));
                }
                Inbound::Notification { .. } => {}
                Inbound::Exited { drained, .. } => {
                    let _ = drained.send(());
                    return;
                }
            }
        }
    });
    let started = server
        .request(
            "initialize",
            json!({
                "clientInfo": { "name": "fastctx", "title": "FastCtx World", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "experimentalApi": false },
            }),
        )
        .await
        .and_then(|answer| server.notify("initialized", json!({})).map(|()| answer));
    let said = match started {
        Ok(answer) => {
            // `<client>/<version> (<system>) …`.
            let version = answer["userAgent"]
                .as_str()
                .and_then(|agent| agent.split_whitespace().next())
                .and_then(|first| first.split_once('/'))
                .map(|(_, version)| version.to_string());
            Ok(ask(&server, version, limits).await)
        }
        Err(error) => Err(format!("starting the Codex app-server {}: {error}", launch.binary.display())),
    };
    server.kill();
    server.wait().await;
    let _ = pump.await;
    said
}

async fn ask(server: &AppServer, version: Option<String>, limits: bool) -> Said {
    let who = super::account::identity(server).await;
    let limits = match &who {
        Fact::Known { value } if value.logged_in && limits => {
            let at = Timestamp::now();
            Some(server.request("account/rateLimits/read", json!({})).await.map(|answer| (at, answer)))
        }
        _ => None,
    };
    let models = match super::agents::models(server).await {
        Ok(models) => Fact::known(models),
        Err(error) => Fact::failed(error),
    };
    Said { version, who, limits, models }
}
