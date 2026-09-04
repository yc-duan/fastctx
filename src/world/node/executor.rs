//! Executes direct calls that arrive from other members on this machine, with this member's
//! own environment, budgets carried by the caller, and a cancel path from the hub.

use crate::model::ToolResponse;
use crate::session::SessionContext;
use crate::world::client::WorldClient;
use crate::world::envelope::{Header, Opened};
use crate::world::messages::{self, Call, CallResult, WireResponse, kind};
use parking_lot::Mutex;
use std::collections::HashMap;

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Runs one call at a time per hub request id and answers on the same id.
pub(crate) struct Executor {
    client: Arc<WorldClient>,
    session: Arc<SessionContext>,
    running: Mutex<HashMap<u64, CancellationToken>>,
}

impl Executor {
    pub(crate) fn new(client: Arc<WorldClient>, session: Arc<SessionContext>) -> Arc<Self> {
        Arc::new(Self {
            client,
            session,
            running: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn running_calls(&self) -> usize {
        self.running.lock().len()
    }

    /// Starts a call in the background; the answer goes back through the client.
    pub(crate) fn spawn_call(self: &Arc<Self>, hub_id: Option<u64>, opened: Opened) {
        let executor = Arc::clone(self);
        tokio::spawn(async move {
            executor.run_call(hub_id, opened).await;
        });
    }

    pub(crate) fn cancel(&self, hub_id: u64) {
        if let Some(token) = self.running.lock().get(&hub_id) {
            token.cancel();
        }
    }

    async fn run_call(&self, hub_id: Option<u64>, opened: Opened) {
        let Some(hub_id) = hub_id else {
            super::log("a call arrived without a request id; nothing to answer");
            return;
        };
        let from = opened.header.from.clone();
        let started = Instant::now();
        let response = match self.prepare(&opened) {
            Ok((call, session)) => {
                let token = CancellationToken::new();
                self.running.lock().insert(hub_id, token.clone());
                let response = execute(call, session, token.clone()).await;
                self.running.lock().remove(&hub_id);
                response
            }
            Err(error) => ToolResponse::error(error),
        };
        let result = CallResult {
            node: self.client.name(),
            response: WireResponse::from(&response),
            elapsed_ms: started.elapsed().as_millis() as u64,
        };
        let header = Header::new(kind::CALL_RESULT, &self.client.name(), &from, 0);
        if let Err(error) = self.client.send_answer(hub_id, header, &result, true) {
            super::log(format!("cannot answer the call from \"{from}\": {error}"));
        }
        self.audit(&from, &opened, &response, started.elapsed());
    }

    fn prepare(&self, opened: &Opened) -> Result<(Call, Arc<SessionContext>), String> {
        if !opened.encrypted {
            return Err("forbidden: calls must be encrypted under the World key.".to_string());
        }
        let from = &opened.header.from;
        let member = self.client.member(from).ok_or_else(|| {
            format!("forbidden: \"{from}\" is not a verified member of this World.")
        })?;
        let _ = member;
        let call: Call = messages::decode(&opened.body, kind::CALL)?;
        let me = self.client.name();
        if !self
            .client
            .grants
            .read()
            .allows(from, &call.verb, &me, &self.client.own_tags())
        {
            return Err(format!(
                "forbidden: node \"{me}\" does not allow {} for \"{from}\".",
                call.verb
            ));
        }
        let cwd = match &call.cwd {
            Some(cwd) => {
                let path = crate::paths::parse_input_path(cwd);
                if !path.is_absolute() {
                    return Err(
                        "The cwd for a remote call must be an absolute path on the target machine."
                            .to_string(),
                    );
                }
                path
            }
            None => self.session.control_paths.home.clone(),
        };
        let budget_variable = match call.verb.as_str() {
            "inspect_local_file" => crate::budget::READ_TOKEN_BUDGET_ENV,
            "grep" => crate::budget::GREP_TOKEN_BUDGET_ENV,
            "glob" => crate::budget::GLOB_TOKEN_BUDGET_ENV,
            "run" => crate::budget::RUN_TOKEN_BUDGET_ENV,
            _ => crate::budget::GLOBAL_TOKEN_BUDGET_ENV,
        };
        let mut overrides = Vec::new();
        if let Some(global) = call.budget.global {
            overrides.push((
                crate::budget::GLOBAL_TOKEN_BUDGET_ENV.to_string(),
                global.to_string(),
            ));
        }
        if let Some(tool) = call.budget.tool {
            overrides.push((budget_variable.to_string(), tool.to_string()));
        }
        let session = SessionContext::for_remote_call(&self.session, cwd, overrides);
        Ok((call, session))
    }

    fn audit(&self, from: &str, opened: &Opened, response: &ToolResponse, elapsed: Duration) {
        let verb = opened.header.verb.clone().unwrap_or_default();
        let summary = response
            .content
            .first()
            .map(|block| match block {
                crate::model::ToolContent::Text(text) => text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect::<String>(),
                crate::model::ToolContent::Image { .. } => "<image>".to_string(),
            })
            .unwrap_or_default();
        let line = serde_json::json!({
            "at": crate::world::now_rfc3339(),
            "principal": from,
            "verb": verb,
            "n": opened.header.n,
            "outcome": if response.is_error { "error" } else { "ok" },
            "summary": summary,
            "elapsed_ms": elapsed.as_millis() as u64,
        });
        let directory = &self.client.paths.audit_dir;
        if std::fs::create_dir_all(directory).is_err() {
            return;
        }
        let day = crate::world::now_rfc3339()
            .chars()
            .take(10)
            .collect::<String>();
        let path = directory.join(format!("{day}.jsonl"));
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

async fn execute(
    call: Call,
    session: Arc<SessionContext>,
    token: CancellationToken,
) -> ToolResponse {
    let timeout = Duration::from_millis(call.timeout_ms.clamp(1_000, 600_000));
    let verb = call.verb.clone();
    let work_token = token.clone();
    let work = tokio::task::spawn_blocking(move || run_tool(&call, &session, &work_token));
    match tokio::time::timeout(timeout, work).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            ToolResponse::error(format!("Internal tool failure on this node: {error}"))
        }
        Err(_) => {
            token.cancel();
            ToolResponse::error(format!(
                "{verb} did not finish within {} s on this node and was stopped.",
                timeout.as_secs()
            ))
        }
    }
}

fn run_tool(call: &Call, session: &Arc<SessionContext>, token: &CancellationToken) -> ToolResponse {
    let args = call.args.clone();
    session.activate(|| match call.verb.as_str() {
        "inspect_local_file" => match serde_json::from_value::<crate::read_tool::ReadRequest>(args)
        {
            Ok(request) => crate::read_tool::read_file(request),
            Err(error) => {
                ToolResponse::error(format!("Invalid inspect_local_file arguments: {error}"))
            }
        },
        "grep" => match serde_json::from_value::<crate::grep_tool::GrepRequest>(args) {
            Ok(request) => crate::grep_tool::grep_files(request, token.clone()),
            Err(error) => ToolResponse::error(format!("Invalid grep arguments: {error}")),
        },
        "glob" => match serde_json::from_value::<crate::glob_tool::GlobRequest>(args) {
            Ok(request) => crate::glob_tool::glob_files(request, token.clone()),
            Err(error) => ToolResponse::error(format!("Invalid glob arguments: {error}")),
        },
        "replace" => match serde_json::from_value::<crate::edit::ReplaceRequest>(args) {
            Ok(request) => {
                let limit = match crate::control::settings::load(&session.control_paths)
                    .and_then(|settings| settings.replace_file_limit_mib())
                {
                    Ok(limit) => limit,
                    Err(error) => {
                        return ToolResponse::error(format!(
                            "Cannot use replace settings on this node: {error}."
                        ));
                    }
                };
                crate::edit::ReplaceService::new().replace_with_limit(request, limit)
            }
            Err(error) => ToolResponse::error(format!("Invalid replace arguments: {error}")),
        },
        "run" => match serde_json::from_value::<crate::shell::RunRequest>(args) {
            Ok(request) => crate::shell::FastShell::with_session(Arc::clone(session))
                .run_until_cancelled(request, || token.is_cancelled()),
            Err(error) => ToolResponse::error(format!("Invalid run arguments: {error}")),
        },
        other => ToolResponse::error(format!("This node does not execute \"{other}\".")),
    })
}
