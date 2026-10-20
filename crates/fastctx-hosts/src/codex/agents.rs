//! Discovery through the very app-server used for the World's sessions.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use fastctx_types::agents::{AgentFacts, AgentModel, AgentProgram, Fact, LaunchCheck, ProbeTime, ReasoningEffort};
use serde::Deserialize;
use serde_json::json;

use super::{CodexHost, launch, server::AppServer};
use crate::agents::{elapsed, measured};

impl CodexHost {
    /// Probes commands without starting a thread, then asks this node's
    /// app-server for its catalog. No credential files are read here.
    pub async fn agents(self: &Arc<Self>) -> AgentFacts {
        let host = self.clone();
        let mut facts = match tokio::task::spawn_blocking(move || host.command_facts()).await {
            Ok(facts) => facts,
            Err(error) => return empty(Fact::failed(format!("probing Codex: {error}"))),
        };
        // A busy app-server can still answer even when its next launch's
        // configuration is broken. Report that running process separately.
        if !matches!(facts.program, Fact::Known { value: Some(_) }) && self.lock().server.is_none() {
            return facts;
        }
        let start = Instant::now();
        let server = self.server().await;
        facts.timings.push(ProbeTime { step: "app_server".into(), elapsed_ms: elapsed(start) });
        match server {
            Ok(server) => {
                facts.launch = match &self.lock().server {
                    Some((current, launch)) if Arc::ptr_eq(current, &server) => {
                        Fact::known(LaunchCheck::CodexAppServer { program: launch.binary.clone(), pid: server.pid() })
                    }
                    _ => Fact::failed("the app-server stopped during discovery"),
                };
                let start = Instant::now();
                facts.models = match models(&server).await {
                    Ok(mut models) => {
                        windows(&self.codex_home, &mut models);
                        Fact::known(models)
                    }
                    Err(error) => Fact::failed(error),
                };
                facts.timings.push(ProbeTime { step: "model/list".into(), elapsed_ms: elapsed(start) });
                let start = Instant::now();
                facts.account = super::account::account(&server).await;
                facts.timings.push(ProbeTime { step: "account/read".into(), elapsed_ms: elapsed(start) });
                facts.limits = match &facts.account {
                    Fact::Unavailable { reason } => Fact::unavailable(reason.clone()),
                    _ => {
                        let start = Instant::now();
                        let limits = super::account::limits(&server).await;
                        facts
                            .timings
                            .push(ProbeTime { step: "account/rateLimits/read".into(), elapsed_ms: elapsed(start) });
                        limits
                    }
                };
            }
            Err(error) => {
                facts.launch = Fact::failed(error);
                facts.models = Fact::unavailable("the Codex app-server could not be reached");
            }
        }
        facts
    }

    fn command_facts(&self) -> AgentFacts {
        let start = Instant::now();
        let path = self.env.iter().find(|(name, _)| name.eq_ignore_ascii_case("PATH")).map(|(_, value)| value);
        let program = match launch::discover_for(&self.codex_home, self.program_home.as_deref(), path) {
            Ok(Some(launch)) => match launch.binary.try_exists() {
                Ok(exists) => Fact::known(Some(AgentProgram { path: launch.binary, source: launch.source, exists })),
                Err(error) => Fact::failed(format!("checking {}: {error}", launch.binary.display())),
            },
            Ok(None) => Fact::known(None),
            Err(error) => Fact::failed(error),
        };
        let mut facts = empty(program);
        facts.timings.push(ProbeTime { step: "resolve".into(), elapsed_ms: elapsed(start) });
        if let Fact::Known { value: Some(program) } = &facts.program {
            facts.version = measured(&mut facts.timings, "--version", || {
                crate::agents::version(&program.path, self.workdir.parent().expect("World directory"), &self.env)
            });
            facts.login = measured(&mut facts.timings, "login status", || {
                crate::agents::login(&program.path, self.workdir.parent().expect("World directory"), &self.env)
            });
        }
        facts
    }
}

fn empty(program: Fact<Option<AgentProgram>>) -> AgentFacts {
    AgentFacts {
        agent: "codex".parse().expect("agent name"),
        home: None,
        account: Fact::unavailable("no selected Codex program"),
        limits: Fact::unavailable("no selected Codex program"),
        program,
        version: Fact::unavailable("no selected Codex program"),
        launch: Fact::unavailable("no selected Codex program"),
        login: Fact::unavailable("no selected Codex program"),
        models: Fact::unavailable("no selected Codex program"),
        integration: Fact::unavailable("the node supplies its apply status"),
        // The World starts Codex threads with approvals never asked.
        unattended: Fact::known(true),
        timings: Vec::new(),
        drift: None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    data: Vec<Model>,
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    id: String,
    model: String,
    display_name: String,
    description: String,
    is_default: bool,
    default_reasoning_effort: String,
    supported_reasoning_efforts: Vec<Effort>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Effort {
    reasoning_effort: String,
    description: String,
}

/// Each model's context window and the most it may be set to, as the catalog
/// Codex keeps in its home from its service lists them: `model/list` does
/// not say. A catalog it has not written yet leaves them unsaid.
fn windows(home: &Path, models: &mut [AgentModel]) {
    #[derive(Deserialize)]
    struct Catalog {
        models: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        slug: String,
        context_window: Option<u64>,
        max_context_window: Option<u64>,
    }
    let Ok(bytes) = std::fs::read(home.join("models_cache.json")) else { return };
    let Ok(catalog) = serde_json::from_slice::<Catalog>(&bytes) else { return };
    for model in models {
        if let Some(entry) = catalog.models.iter().find(|entry| entry.slug == model.model) {
            model.context_window = entry.context_window.or(entry.max_context_window);
            model.max_context_window = entry.max_context_window.or(entry.context_window);
        }
    }
}

pub(super) async fn models(server: &AppServer) -> Result<Vec<AgentModel>, String> {
    let mut models = Vec::new();
    let mut cursor = None::<String>;
    loop {
        let answer = server.request("model/list", json!({ "includeHidden": false, "cursor": cursor })).await?;
        let page: Page = serde_json::from_value(answer).map_err(|error| format!("reading model/list: {error}"))?;
        models.extend(page.data.into_iter().map(|model| {
            AgentModel {
                id: model.id,
                model: model.model,
                display_name: model.display_name,
                description: model.description,
                is_default: model.is_default,
                default_reasoning_effort: model.default_reasoning_effort,
                reasoning_efforts: model
                    .supported_reasoning_efforts
                    .into_iter()
                    .map(|effort| ReasoningEffort { effort: effort.reasoning_effort, description: effort.description })
                    .collect(),
                context_window: None,
                max_context_window: None,
            }
        }));
        cursor = page.next_cursor;
        if cursor.is_none() {
            return Ok(models);
        }
    }
}
