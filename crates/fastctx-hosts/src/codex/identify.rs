//! Asking a Codex login who is logged in there, its limits and its models,
//! through its own app-server, and leaving nothing running for the asking:
//! the app-server already serving the login is asked when there is one;
//! otherwise one is started for the questions and stopped once answered.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_types::agents::{AgentModel, Fact};
use fastctx_types::logins::Identity;
use jiff::Timestamp;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::launch::Launch;
use super::server::{AppServer, Inbound};
use super::{CodexHost, Log};

/// What a login's app-server said of it.
#[derive(Debug)]
pub struct Said {
    /// The program's version, from its answer to `initialize`.
    pub version: Option<String>,
    pub who: Fact<Identity>,
    /// The answer of `account/rateLimits/read`, and when it came; none
    /// when no one is logged in, or the limits were not asked for.
    pub limits: Option<Result<(Timestamp, Value), String>>,
    pub models: Fact<Vec<AgentModel>>,
}

/// What a login's app-server said of it on starting for the host's own
/// work: what identifying the login asks, put to the server that is running
/// anyway, so nothing is started for it.
#[derive(Debug)]
pub struct SaidReport {
    /// The login: the home the app-server runs for.
    pub home: PathBuf,
    /// The program that said it.
    pub launch: Launch,
    pub at: Timestamp,
    pub said: Said,
}

impl CodexHost {
    /// Asks this host's app-server when it is running now, its usage limits
    /// too with `limits`, and says which program it runs; `None` when it is
    /// not, and then nothing is started.
    pub async fn ask_running(self: &Arc<Self>, limits: bool) -> Option<(Launch, Said)> {
        let (server, launch, version) = {
            let state = self.lock();
            let (server, launch) = state.server.as_ref()?;
            (server.clone(), launch.clone(), state.version.clone())
        };
        let said = ask(&server, version, limits, true).await;
        if let Some(Ok((at, answer))) = &said.limits {
            self.limits_heard(*at, super::account::LimitsHow::Read(answer.clone()));
        }
        Some((launch, said))
    }

    /// The program this host starts, as it would start it now.
    pub fn launch(&self) -> Result<Option<Launch>, String> {
        let path = self.env.iter().find(|(name, _)| name.eq_ignore_ascii_case("PATH")).map(|(_, value)| value);
        super::launch::discover_for(&self.codex_home, self.program_home.as_deref(), path)
    }

    /// Where this host tells what its app-server says of its login each
    /// time one starts.
    pub fn report_said_to(&self, sink: mpsc::UnboundedSender<SaidReport>) {
        *self.said.lock().expect("never poisoned") = Some(sink);
    }

    /// Asks `server`, just started from `launch` for this host's work, who
    /// is logged in and its models, never its limits, and tells it.
    pub(super) fn tell_said(&self, server: Arc<AppServer>, launch: Launch, version: Option<String>) {
        let Some(sink) = self.said.lock().expect("never poisoned").clone() else { return };
        let home = self.codex_home.clone();
        let owner = self.limits_owner.clone();
        let generation = {
            let mut owner = owner.lock().expect("never poisoned");
            owner.generation += 1;
            owner.identity = None;
            owner.generation
        };
        tokio::spawn(async move {
            let at = Timestamp::now();
            let said = ask(&server, version, false, true).await;
            if let Fact::Known { value } = &said.who {
                let mut owner = owner.lock().expect("never poisoned");
                if owner.generation != generation {
                    return;
                }
                owner.identity = Some((at, value.clone()));
            }
            let _ = sink.send(SaidReport { home, launch, at, said });
        });
    }
}

/// The program's version in an answer to `initialize`, whose user agent is
/// `<client>/<version> (<system>) …`.
pub(super) fn version_of(answer: &Value) -> Option<String> {
    answer["userAgent"]
        .as_str()
        .and_then(|agent| agent.split_whitespace().next())
        .and_then(|first| first.split_once('/'))
        .map(|(_, version)| version.to_string())
}

/// Starts `launch` for the Codex home `home`, asks it, its usage limits
/// too with `limits` and its models with `models`, and stops it.
pub async fn ask_once(
    launch: &Launch,
    home: &Path,
    cwd: &Path,
    env: &[(OsString, OsString)],
    log: Log,
    limits: bool,
    models: bool,
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
        Ok(answer) => Ok(ask(&server, version_of(&answer), limits, models).await),
        Err(error) => Err(format!("starting the Codex app-server {}: {error}", launch.binary.display())),
    };
    server.kill();
    server.wait().await;
    let _ = pump.await;
    said
}

async fn ask(server: &AppServer, version: Option<String>, limits: bool, models: bool) -> Said {
    let who = super::account::identity(server).await;
    let limits = match &who {
        Fact::Known { value } if value.logged_in && limits => {
            let at = Timestamp::now();
            Some(server.request("account/rateLimits/read", json!({})).await.map(|answer| (at, answer)))
        }
        _ => None,
    };
    let models = if !models {
        Fact::unavailable("the models are listed for the workbench")
    } else {
        match super::agents::models(server).await {
            Ok(models) => Fact::known(models),
            Err(error) => Fact::failed(error),
        }
    };
    Said { version, who, limits, models }
}
