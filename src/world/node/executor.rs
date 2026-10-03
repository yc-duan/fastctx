//! Executes direct calls that arrive from other members on this machine, with this member's
//! own environment, budgets carried by the caller, and a cancel path from the hub.
//!
//! The same `execute` serves a World call that names this very machine (`dispatch.rs`), so a
//! verb behaves identically whether it was asked for from here or from another member.

use crate::model::ToolResponse;
use crate::session::SessionContext;
use crate::world::client::WorldClient;
use crate::world::envelope::{Header, Opened};
use crate::world::messages::{self, Call, CallOrigin, CallResult, WireResponse, kind};
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// The longest a tool-originated call may run on this node; the host stopped waiting long
/// before this, so the ceiling only bounds a runaway.
const TOOL_CALL_CEILING_MS: u64 = 600_000;
/// Added to a verb's own timeout before this node gives up on it: a `run` that stops its
/// command at the timeout still has to terminate the process tree and render what it captured,
/// and that rendering, not this node's sentence, is what the caller should get.
pub(crate) const VERB_TIMEOUT_MARGIN: Duration = Duration::from_secs(10);

/// What one verb produced: the rendered response, plus facts the caller's session or a script
/// reads without parsing the head note.
pub(crate) struct VerbOutcome {
    pub(crate) response: ToolResponse,
    pub(crate) facts: BTreeMap<String, serde_json::Value>,
}

impl VerbOutcome {
    pub(crate) fn plain(response: ToolResponse) -> Self {
        Self {
            response,
            facts: BTreeMap::new(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self::plain(ToolResponse::error(message.into()))
    }

    /// An error whose sentence reaches the caller's model as `<node>: <sentence>`.
    pub(crate) fn error_text(message: impl Into<String>) -> Self {
        Self::error(message)
    }
}

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

    /// The daemon's own session: this machine's environment, settings, and home directory.
    pub(crate) fn session(&self) -> Arc<SessionContext> {
        Arc::clone(&self.session)
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
        // The caller's own request id sits in the authenticated header; the answer echoes it
        // there so the caller can refuse an answer the hub matched to the wrong request.
        let Some(request_id) = opened.header.id else {
            super::log("a call arrived without a caller request id; nothing to answer");
            return;
        };
        let from = opened.header.from.clone();
        let started = Instant::now();
        let outcome = match self.prepare(&opened) {
            Ok((call, session)) => {
                let token = CancellationToken::new();
                self.running.lock().insert(hub_id, token.clone());
                let outcome = execute(&self.client, call, session, token.clone()).await;
                self.running.lock().remove(&hub_id);
                outcome
            }
            Err(error) => VerbOutcome::error(error),
        };
        let result = CallResult {
            node: self.client.name(),
            response: WireResponse::from(&outcome.response),
            elapsed_ms: started.elapsed().as_millis() as u64,
            grant_revision: self.client.grant_revision(),
            facts: outcome.facts,
        };
        let header =
            Header::new(kind::CALL_RESULT, &self.client.name(), &from, 0).with_id(request_id);
        if let Err(error) = self.client.send_answer(
            hub_id,
            header,
            &result,
            opened.header.verb.as_deref().unwrap_or(""),
            Duration::from_secs(opened.header.wait_s.unwrap_or(0)),
        ) {
            super::log(format!("cannot answer the call from \"{from}\": {error}"));
        }
        self.audit(&from, &opened, &outcome.response, started.elapsed());
    }

    fn prepare(&self, opened: &Opened) -> Result<(Call, Arc<SessionContext>), String> {
        // These sentences travel back inside the call result and reach the caller's model as
        // `<node>: <sentence>`, so they read as prose, not as error codes.
        let from = &opened.header.from;
        if self.client.member(from).is_none() {
            return Err(format!(
                "\"{from}\" is not a verified member of this World; nothing ran."
            ));
        }
        let call: Call = messages::decode(&opened.body, kind::CALL)?;
        let me = self.client.name();
        // The caller's revision arrived inside the signed body; if it is newer than this node's,
        // a narrowing may exist that this node has not seen, and honouring the older set could
        // execute something already withdrawn.
        if self.client.raise_grant_floor(call.grant_revision)? {
            return Err(format!(
                "Node \"{me}\" holds grant revision {} but revision {} is in force; it is fetching the newer set. Retry shortly.",
                self.client.grant_revision(),
                self.client.state_snapshot().grant_floor
            ));
        }
        let grant_verb = grant_verb(&call.verb);
        if opened.header.verb.as_deref() != Some(grant_verb) {
            return Err("Call verb differs from signed header".into());
        }
        if call.verb == "copy_receive"
            && call
                .args
                .get("from_node")
                .and_then(serde_json::Value::as_str)
                != Some(from.as_str())
        {
            return Err("Copy source differs from caller".into());
        }
        if !self
            .client
            .grants
            .read()
            .allows(from, grant_verb, &me, &self.client.own_tags())
        {
            return Err(format!(
                "Node \"{me}\" does not allow {grant_verb} for \"{from}\"."
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

/// The grant verb a wire verb is judged by. The receiving side of a copy is still `copy`: the
/// bytes a member writes are the bytes another member sends it.
pub(crate) fn grant_verb(verb: &str) -> &str {
    match verb {
        "copy_receive" => "copy",
        other => other,
    }
}

/// Runs one call under its deadline and cancel token. Blocking verbs run on the blocking pool;
/// `copy` is asynchronous because it drives a data connection while it runs.
pub(crate) async fn execute(
    client: &Arc<WorldClient>,
    call: Call,
    session: Arc<SessionContext>,
    token: CancellationToken,
) -> VerbOutcome {
    let ceiling = match call.origin {
        CallOrigin::Tool => TOOL_CALL_CEILING_MS,
        CallOrigin::Script => crate::shell::MAX_SCRIPT_CALL_MS,
    };
    let timeout = Duration::from_millis(call.timeout_ms.clamp(1_000, ceiling));
    let verb = call.verb.clone();
    let work_token = token.clone();
    let work = async move {
        match call.verb.as_str() {
            "copy" | "copy_receive" => {
                crate::world::copy::execute(client, &call, &session, &work_token, timeout).await
            }
            _ => match tokio::task::spawn_blocking(move || run_verb(&call, &session, &work_token))
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    VerbOutcome::error(format!("Internal tool failure on this node: {error}"))
                }
            },
        }
    };
    match tokio::time::timeout(timeout + VERB_TIMEOUT_MARGIN, work).await {
        Ok(outcome) => outcome,
        Err(_) => {
            token.cancel();
            VerbOutcome::error(format!(
                "{verb} did not finish within {} s on this node and was stopped.",
                timeout.as_secs()
            ))
        }
    }
}

/// Runs one blocking verb with the given session's working directory and budgets.
fn run_verb(call: &Call, session: &Arc<SessionContext>, token: &CancellationToken) -> VerbOutcome {
    let args = call.args.clone();
    session.activate(|| match call.verb.as_str() {
        "inspect_local_file" => match serde_json::from_value::<crate::read_tool::ReadRequest>(args)
        {
            Ok(request) => {
                let path = request.file_path.clone();
                let response = crate::read_tool::read_file(request);
                let facts = if response.is_error {
                    BTreeMap::new()
                } else {
                    crate::world::remote_edit::file_facts(&path)
                };
                VerbOutcome { response, facts }
            }
            Err(error) => {
                VerbOutcome::error(format!("Invalid inspect_local_file arguments: {error}"))
            }
        },
        "grep" => match serde_json::from_value::<crate::grep_tool::GrepRequest>(args) {
            Ok(request) => VerbOutcome::plain(crate::grep_tool::grep_files(request, token.clone())),
            Err(error) => VerbOutcome::error(format!("Invalid grep arguments: {error}")),
        },
        "glob" => match serde_json::from_value::<crate::glob_tool::GlobRequest>(args) {
            Ok(request) => VerbOutcome::plain(crate::glob_tool::glob_files(request, token.clone())),
            Err(error) => VerbOutcome::error(format!("Invalid glob arguments: {error}")),
        },
        "replace" => match serde_json::from_value::<crate::edit::ReplaceRequest>(args) {
            Ok(request) => {
                let limit = match crate::control::settings::load(&session.control_paths)
                    .and_then(|settings| settings.replace_file_limit_mib())
                {
                    Ok(limit) => limit,
                    Err(error) => {
                        return VerbOutcome::error(format!(
                            "Cannot use replace settings on this node: {error}."
                        ));
                    }
                };
                VerbOutcome::plain(
                    crate::edit::ReplaceService::new().replace_with_limit(request, limit),
                )
            }
            Err(error) => VerbOutcome::error(format!("Invalid replace arguments: {error}")),
        },
        "run" => match serde_json::from_value::<crate::shell::RunRequest>(args) {
            Ok(request) => {
                let ceiling = match call.origin {
                    CallOrigin::Tool => crate::shell::MAX_BLOCKING_CALL_MS,
                    CallOrigin::Script => crate::shell::MAX_SCRIPT_CALL_MS,
                };
                let explicit_cwd = request.cwd.is_some();
                let outcome = crate::shell::FastShell::with_session(Arc::clone(session))
                    .run_bounded(request, ceiling, || token.is_cancelled());
                let mut facts = BTreeMap::new();
                if let Some(code) = outcome.exit_code {
                    facts.insert("exit_code".to_string(), serde_json::json!(code));
                }
                let mut response = outcome.response;
                // A remote command without cwd ran in this machine's home directory, and the
                // caller has no other way to learn where that is.
                if !explicit_cwd && !response.is_error {
                    let cwd = crate::paths::display_path(session.environment.cwd());
                    crate::head_note::append_fact(&mut response, &format!("in {cwd}"));
                    facts.insert("cwd".to_string(), serde_json::json!(cwd));
                }
                VerbOutcome { response, facts }
            }
            Err(error) => VerbOutcome::error(format!("Invalid run arguments: {error}")),
        },
        "remote_write" => match serde_json::from_value(args) {
            Ok(request) => crate::world::remote_edit::write(request),
            Err(error) => VerbOutcome::error(format!("Invalid remote_write arguments: {error}")),
        },
        "remote_edit" => match serde_json::from_value(args) {
            Ok(request) => crate::world::remote_edit::edit(request, &session.control_paths),
            Err(error) => VerbOutcome::error(format!("Invalid remote_edit arguments: {error}")),
        },
        other => VerbOutcome::error(format!("This node does not execute \"{other}\".")),
    })
}
