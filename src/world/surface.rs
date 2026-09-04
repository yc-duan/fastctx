//! The World-mode tool surface: `node` on the file tools and `run`, the `nodes` machine map,
//! single-node head notes that name the machine, and fleet aggregation for fan-out reads.
//!
//! A call without `node` takes exactly the 1.0 local path. A call with `node` naming this
//! machine runs locally too, but answers as a World response (its head note names the
//! node). Everything else goes through the node daemon's hub link.

use crate::budget::{GLOBAL_TOKEN_BUDGET_ENV, estimate_tokens, tool_token_budget};
use crate::head_note::{HeadMetric, HeadNote};
use crate::model::{ToolContent, ToolResponse};
use crate::server::FastCtxServer;
use crate::server_support::into_mcp_result;
use crate::world::client::{NodeOutcome, NodeView, WorldClient};
use crate::world::members::Selector;
use crate::world::messages::{Call, CallBudget};
use rmcp::RoleServer;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// The `node` parameter on the read-only file tools.
pub(crate) const NODE_PARAMETER_MULTI: &str = "Machines to run this on: one node name, several, \"tag:<tag>\", or \"all\". Omit for this machine. With several, results are grouped per machine and identical outputs are shown once.";
/// The `node` parameter on `replace` and `run`.
pub(crate) const NODE_PARAMETER_SINGLE: &str =
    "The one machine to run this on. Omit for this machine.";
/// What a World call on an unenrolled or degraded engine says.
pub(crate) const NODE_SERVICE_NOT_RUNNING: &str =
    "The FastCtx node service is not running on this machine; run 'fastctx node status'.";
/// Default deadline for a remote read when the tool itself has no timeout parameter.
const DEFAULT_REMOTE_TIMEOUT: Duration = Duration::from_secs(120);
/// Node names listed inline in a group header before the rest is counted.
const GROUP_NAMES_INLINE: usize = 20;

/// The arguments of a World-mode tool call: the base tool's own object plus the optional
/// `node` selector. The published schema is the base schema with `node` added, so this type
/// only needs to be an object.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WorldArguments(pub(crate) serde_json::Map<String, serde_json::Value>);

impl JsonSchema for WorldArguments {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorldArguments".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "object" })
    }
}

impl WorldArguments {
    /// Splits the selector off; `None` means this machine, the 1.0 path.
    pub(crate) fn split(self) -> Result<(Option<Selector>, serde_json::Value), String> {
        let mut map = self.0;
        let selector = match map.remove("node") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(name)) => Some(Selector::parse_items(&[name])?),
            Some(serde_json::Value::Array(items)) => {
                let items = items
                    .into_iter()
                    .map(|item| match item {
                        serde_json::Value::String(name) => Ok(name),
                        other => Err(format!(
                            "Invalid node selector entry {other}: expected a machine name, \"tag:<tag>\", or \"all\"."
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Some(Selector::parse_items(&items)?)
            }
            Some(other) => {
                return Err(format!(
                    "Invalid node value {other}: expected a machine name or a list of names."
                ));
            }
        };
        Ok((selector, serde_json::Value::Object(map)))
    }
}

/// Adds the `node` property to a published tool schema, declared as a string array (a bare
/// string is accepted at call time, like `glob.pattern`).
pub(crate) fn add_node_property(
    schema: &serde_json::Map<String, serde_json::Value>,
    description: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut schema = schema.clone();
    let properties = schema
        .entry("properties")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let serde_json::Value::Object(properties) = properties {
        properties.insert(
            "node".to_string(),
            serde_json::json!({
                "type": "array",
                "items": { "type": "string" },
                "description": description,
            }),
        );
    }
    schema
}

/// Operating systems a requirement can name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OsRequirement {
    /// Linux, including WSL2 nodes.
    Linux,
    /// macOS.
    Macos,
    /// Windows.
    Windows,
}

impl OsRequirement {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
        }
    }
}

/// CPU architectures a requirement can name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq)]
pub enum ArchRequirement {
    /// 64-bit x86.
    #[serde(rename = "x86_64")]
    X86_64,
    /// 64-bit ARM.
    #[serde(rename = "aarch64")]
    Aarch64,
}

impl ArchRequirement {
    const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }
}

/// Requirements a machine must meet (`design-objects.md` §2.2).
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    /// Operating system the machine must run.
    pub os: Option<OsRequirement>,
    /// CPU architecture the machine must have.
    pub arch: Option<ArchRequirement>,
    /// At least this many CPUs.
    #[schemars(range(min = 1))]
    pub cpus: Option<u32>,
    /// At least this many gigabytes of memory.
    pub memory_gb: Option<f32>,
    /// At least this many GPUs.
    #[schemars(range(min = 1))]
    pub gpus: Option<u32>,
    /// Every GPU has at least this many gigabytes of memory.
    pub gpu_memory_gb: Option<f32>,
    /// Every listed tag must be present.
    pub tags: Option<Vec<String>>,
    /// Only consider these machines.
    pub nodes: Option<Vec<String>>,
}

impl Requirements {
    pub(crate) fn matches(&self, node: &NodeView) -> bool {
        if let Some(os) = self.os
            && !node.os.eq_ignore_ascii_case(os.as_str())
        {
            return false;
        }
        if let Some(arch) = self.arch
            && !node.arch.eq_ignore_ascii_case(arch.as_str())
        {
            return false;
        }
        if let Some(tags) = &self.tags
            && !tags.iter().all(|tag| node.tags.contains(tag))
        {
            return false;
        }
        if let Some(nodes) = &self.nodes
            && !nodes.contains(&node.name)
        {
            return false;
        }
        let inventory = node.inventory.as_ref();
        if let Some(cpus) = self.cpus
            && inventory.is_none_or(|inventory| inventory.cpus < cpus)
        {
            return false;
        }
        if let Some(memory) = self.memory_gb
            && inventory.is_none_or(|inventory| inventory.memory_gb < memory)
        {
            return false;
        }
        if let Some(gpus) = self.gpus
            && inventory.is_none_or(|inventory| (inventory.gpus.len() as u32) < gpus)
        {
            return false;
        }
        if let Some(gpu_memory) = self.gpu_memory_gb
            && inventory.is_none_or(|inventory| {
                inventory.gpus.is_empty()
                    || inventory.gpus.iter().any(|gpu| gpu.memory_gb < gpu_memory)
            })
        {
            return false;
        }
        true
    }

    fn is_empty(&self) -> bool {
        self.os.is_none()
            && self.arch.is_none()
            && self.cpus.is_none()
            && self.memory_gb.is_none()
            && self.gpus.is_none()
            && self.gpu_memory_gb.is_none()
            && self.tags.is_none()
            && self.nodes.is_none()
    }
}

/// Parameters of the `nodes` tool.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodesRequest {
    /// Keep only machines meeting these requirements.
    pub need: Option<Requirements>,
    /// Keep only machines carrying every listed tag.
    pub tags: Option<Vec<String>>,
    /// true keeps online machines, false keeps offline ones; omit for both.
    pub online: Option<bool>,
}

#[tool_router(router = world_tool_router, vis = "pub(crate)")]
impl FastCtxServer {
    #[tool(
        name = "inspect_local_file",
        description = "Inspect local files.",
        annotations(
            title = "Inspect local file",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn world_inspect_local_file(
        &self,
        Parameters(arguments): Parameters<WorldArguments>,
    ) -> CallToolResult {
        let (selector, value) = match arguments.split() {
            Ok(split) => split,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        match selector {
            None => match serde_json::from_value(value) {
                Ok(request) => self.local_inspect_local_file(request).await,
                Err(error) => invalid_arguments("inspect_local_file", error),
            },
            Some(selector) => {
                self.world_call(
                    "inspect_local_file",
                    selector,
                    value,
                    crate::budget::READ_TOKEN_BUDGET_ENV,
                    false,
                )
                .await
            }
        }
    }

    #[tool(
        name = "grep",
        description = "Search file contents.",
        annotations(
            title = "Search file contents",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn world_grep(
        &self,
        Parameters(arguments): Parameters<WorldArguments>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let (selector, value) = match arguments.split() {
            Ok(split) => split,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        match selector {
            None => match serde_json::from_value(value) {
                Ok(request) => self.local_grep(request, context).await,
                Err(error) => invalid_arguments("grep", error),
            },
            Some(selector) => {
                self.world_call(
                    "grep",
                    selector,
                    value,
                    crate::budget::GREP_TOKEN_BUDGET_ENV,
                    false,
                )
                .await
            }
        }
    }

    #[tool(
        name = "glob",
        description = "Match file paths.",
        annotations(
            title = "Match file paths",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn world_glob(
        &self,
        Parameters(arguments): Parameters<WorldArguments>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let (selector, value) = match arguments.split() {
            Ok(split) => split,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        match selector {
            None => match serde_json::from_value(value) {
                Ok(request) => self.local_glob(request, context).await,
                Err(error) => invalid_arguments("glob", error),
            },
            Some(selector) => {
                self.world_call(
                    "glob",
                    selector,
                    value,
                    crate::budget::GLOB_TOKEN_BUDGET_ENV,
                    false,
                )
                .await
            }
        }
    }

    #[tool(
        name = "replace",
        description = "Batch find-and-replace across a file or directory.",
        annotations(
            title = "Batch replace file contents",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn world_replace(
        &self,
        Parameters(arguments): Parameters<WorldArguments>,
    ) -> CallToolResult {
        let (selector, value) = match arguments.split() {
            Ok(split) => split,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        match selector {
            None => match serde_json::from_value(value) {
                Ok(request) => self.local_replace(request).await,
                Err(error) => invalid_arguments("replace", error),
            },
            Some(selector) => {
                self.world_call("replace", selector, value, GLOBAL_TOKEN_BUDGET_ENV, true)
                    .await
            }
        }
    }

    #[tool(
        name = "run",
        description = "Run a bash command.",
        annotations(
            title = "Run bash command",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn world_run(
        &self,
        Parameters(arguments): Parameters<WorldArguments>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let (selector, value) = match arguments.split() {
            Ok(split) => split,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        match selector {
            None => match serde_json::from_value(value) {
                Ok(request) => self.local_run(request, context).await,
                Err(error) => invalid_arguments("run", error),
            },
            Some(selector) => {
                self.world_call(
                    "run",
                    selector,
                    value,
                    crate::budget::RUN_TOKEN_BUDGET_ENV,
                    true,
                )
                .await
            }
        }
    }

    #[tool(
        name = "nodes",
        description = "List the machines in this World: name, OS and architecture, online state, CPUs, memory, GPUs, tags, and how each is linked to the hub. Pass need to keep only machines that meet requirements (os, arch, cpus, memory_gb, gpus, gpu_memory_gb, tags, nodes); the first line reports how many matched out of how many exist. Machine names are the values other tools accept in node.",
        annotations(
            title = "List World machines",
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn nodes(&self, Parameters(request): Parameters<NodesRequest>) -> CallToolResult {
        let _activity = self.activity.request();
        let Some(client) = self.world.clone() else {
            return into_mcp_result(ToolResponse::error(NODE_SERVICE_NOT_RUNNING));
        };
        let response = list_nodes(&client, request).await;
        let session = Arc::clone(&self.session);
        let status_shell = self.shell.clone();
        into_mcp_result(
            tokio::task::spawn_blocking(move || {
                session.activate(|| {
                    crate::background_status::BackgroundDecorator::new(
                        status_shell.background_status(None),
                        GLOBAL_TOKEN_BUDGET_ENV,
                    )
                    .finish(response)
                })
            })
            .await
            .unwrap_or_else(|error| ToolResponse::error(format!("Internal tool failure: {error}"))),
        )
    }
}

fn invalid_arguments(tool: &str, error: serde_json::Error) -> CallToolResult {
    into_mcp_result(ToolResponse::error(format!(
        "Invalid {tool} arguments: {error}"
    )))
}

impl FastCtxServer {
    /// Runs `verb` on the selected machines and renders the World response.
    async fn world_call(
        &self,
        verb: &str,
        selector: Selector,
        args: serde_json::Value,
        budget_variable: &'static str,
        single_only: bool,
    ) -> CallToolResult {
        let _activity = self.activity.request();
        let Some(client) = self.world.clone() else {
            return into_mcp_result(ToolResponse::error(NODE_SERVICE_NOT_RUNNING));
        };
        if single_only && selector.single_name().is_none() {
            return into_mcp_result(ToolResponse::error(format!(
                "{verb} runs on one machine per call; pass a single node name. Selectors like \"all\" and \"tag:<tag>\" belong to read-only tools and to task scripts."
            )));
        }
        let targets = match client.expand(&selector) {
            Ok(targets) => targets,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        if targets.is_empty() {
            return into_mcp_result(ToolResponse::error(format!(
                "No online machine matches \"{}\". List machines with nodes.",
                selector.describe()
            )));
        }
        let session = Arc::clone(&self.session);
        let budget = {
            let session = Arc::clone(&session);
            tokio::task::spawn_blocking(move || {
                session.activate(|| {
                    let global = crate::budget::token_budget().ok();
                    let tool = tool_token_budget(budget_variable)
                        .ok()
                        .map(|budget| budget.value);
                    CallBudget { global, tool }
                })
            })
            .await
            .unwrap_or_default()
        };
        let timeout = if verb == "run" {
            args.get("timeout_ms")
                .and_then(serde_json::Value::as_u64)
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_REMOTE_TIMEOUT)
        } else {
            DEFAULT_REMOTE_TIMEOUT
        };
        let me = client.name();
        let (local, remote): (Vec<String>, Vec<String>) =
            targets.into_iter().partition(|target| *target == me);
        let mut outcomes = Vec::new();
        if !local.is_empty() {
            outcomes.push(
                run_on_this_machine(&me, verb, args.clone(), Arc::clone(&session), timeout).await,
            );
        }
        if !remote.is_empty() {
            let remote_selector = Selector { items: remote };
            match client
                .call(verb, &remote_selector, args, budget, None, timeout)
                .await
            {
                Ok(remote_outcomes) => outcomes.extend(remote_outcomes),
                Err(error) => return into_mcp_result(ToolResponse::error(error)),
            }
        }
        outcomes.sort_by(|left, right| left.node.cmp(&right.node));
        let verb = verb.to_string();
        let status_shell = self.shell.clone();
        let response = tokio::task::spawn_blocking(move || {
            session.activate(|| {
                let budget_tokens = tool_token_budget(budget_variable)
                    .map(|budget| budget.value)
                    .unwrap_or(crate::budget::DEFAULT_TOKEN_BUDGET);
                let response = if outcomes.len() == 1 {
                    single_node_response(&verb, outcomes.remove(0))
                } else {
                    fleet_response(&verb, outcomes, budget_tokens)
                };
                crate::background_status::BackgroundDecorator::new(
                    status_shell.background_status(None),
                    budget_variable,
                )
                .finish(response)
            })
        })
        .await
        .unwrap_or_else(|error| ToolResponse::error(format!("Internal tool failure: {error}")));
        into_mcp_result(response)
    }
}

/// Executes a World call whose target is this very machine, with the caller's own session.
async fn run_on_this_machine(
    me: &str,
    verb: &str,
    args: serde_json::Value,
    session: Arc<crate::session::SessionContext>,
    timeout: Duration,
) -> NodeOutcome {
    let call = Call {
        verb: verb.to_string(),
        args,
        budget: CallBudget::default(),
        cwd: None,
        timeout_ms: timeout.as_millis() as u64,
    };
    let token = CancellationToken::new();
    let work_token = token.clone();
    let work = tokio::task::spawn_blocking(move || {
        crate::world::node::executor::run_verb(&call, &session, &work_token)
    });
    let response = match tokio::time::timeout(timeout, work).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => ToolResponse::error(format!("Internal tool failure: {error}")),
        Err(_) => {
            token.cancel();
            ToolResponse::error(format!(
                "{verb} did not finish within {} s on this machine and was stopped.",
                timeout.as_secs()
            ))
        }
    };
    NodeOutcome {
        node: me.to_string(),
        status: if response.is_error { "error" } else { "ok" }.to_string(),
        response: Some(response),
        message: None,
    }
}

/// A parsed 1.0 head note: `=== subject (clauses) ===`.
struct ParsedHead {
    subject: String,
    clauses: String,
}

fn parse_head(line: &str) -> Option<ParsedHead> {
    let inner = line.strip_prefix("=== ")?.strip_suffix(" ===")?;
    let inner = inner.strip_suffix(')')?;
    let split = inner.rfind(" (")?;
    Some(ParsedHead {
        subject: inner[..split].to_string(),
        clauses: inner[split + 2..].to_string(),
    })
}

/// The subject a World response shows for one machine.
fn node_subject(verb: &str, subject: &str, node: &str) -> String {
    if verb == "inspect_local_file" {
        format!("{node}:{subject}")
    } else {
        format!("{subject} on {node}")
    }
}

/// Renders one machine's outcome as a World response whose head note names the machine.
fn single_node_response(verb: &str, outcome: NodeOutcome) -> ToolResponse {
    match outcome.response {
        Some(mut response) if !response.is_error => {
            if let Some(ToolContent::Text(text)) = response.content.first_mut() {
                let (first, rest) = text
                    .split_once('\n')
                    .map_or((text.as_str(), None), |(first, rest)| (first, Some(rest)));
                let head = match parse_head(first) {
                    Some(parsed) => format!(
                        "=== {} ({}) ===",
                        node_subject(verb, &parsed.subject, &outcome.node),
                        parsed.clauses
                    ),
                    None => format!("[{}] {first}", outcome.node),
                };
                *text = match rest {
                    Some(rest) => format!("{head}\n{rest}"),
                    None => head,
                };
            }
            response
        }
        Some(response) => {
            let message = response
                .content
                .iter()
                .find_map(|block| match block {
                    ToolContent::Text(text) => Some(text.clone()),
                    ToolContent::Image { .. } => None,
                })
                .unwrap_or_default();
            ToolResponse::error(format!("{}: {message}", outcome.node))
        }
        None => ToolResponse::error(status_line(&outcome)),
    }
}

/// `node-7: unreachable (no heartbeat for 3m)`.
fn status_line(outcome: &NodeOutcome) -> String {
    match &outcome.message {
        Some(message) => format!("{}: {} ({message})", outcome.node, outcome.status),
        None => format!("{}: {}", outcome.node, outcome.status),
    }
}

struct Group {
    nodes: Vec<String>,
    head_clauses: String,
    body: String,
    no_match: bool,
    exit_code: Option<i64>,
}

/// Merges several machines' outcomes into one response (`design-agent-surface.md` §3.2).
fn fleet_response(verb: &str, outcomes: Vec<NodeOutcome>, budget_tokens: usize) -> ToolResponse {
    let total = outcomes.len();
    let mut failures = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut subject: Option<String> = None;
    let mut images_omitted = Vec::new();
    for outcome in outcomes {
        match &outcome.response {
            Some(response) if !response.is_error => {
                let text = response
                    .content
                    .iter()
                    .find_map(|block| match block {
                        ToolContent::Text(text) => Some(text.clone()),
                        ToolContent::Image { .. } => None,
                    })
                    .unwrap_or_default();
                if response
                    .content
                    .iter()
                    .any(|block| matches!(block, ToolContent::Image { .. }))
                {
                    images_omitted.push(outcome.node.clone());
                }
                let (first, body) = text
                    .split_once('\n')
                    .map_or((text.as_str(), ""), |(first, rest)| (first, rest));
                let (clauses, parsed_subject) = match parse_head(first) {
                    Some(parsed) => (parsed.clauses, Some(parsed.subject)),
                    None => (first.to_string(), None),
                };
                if subject.is_none() {
                    subject = parsed_subject;
                }
                let no_match = clauses.starts_with("0 ");
                let exit_code = clauses
                    .split("; ")
                    .find_map(|clause| clause.strip_prefix("exited "))
                    .and_then(|code| code.parse::<i64>().ok());
                match groups
                    .iter_mut()
                    .find(|group| group.head_clauses == clauses && group.body == body)
                {
                    Some(group) => group.nodes.push(outcome.node.clone()),
                    None => groups.push(Group {
                        nodes: vec![outcome.node.clone()],
                        head_clauses: clauses,
                        body: body.to_string(),
                        no_match,
                        exit_code,
                    }),
                }
            }
            Some(response) => {
                let message = response
                    .content
                    .iter()
                    .find_map(|block| match block {
                        ToolContent::Text(text) => {
                            Some(text.lines().next().unwrap_or_default().to_string())
                        }
                        ToolContent::Image { .. } => None,
                    })
                    .unwrap_or_default();
                failures.push((outcome.node.clone(), "error".to_string(), message));
            }
            None => failures.push((
                outcome.node.clone(),
                outcome.status.clone(),
                outcome.message.clone().unwrap_or_default(),
            )),
        }
    }
    groups.sort_by(|left, right| {
        right
            .nodes
            .len()
            .cmp(&left.nodes.len())
            .then_with(|| left.nodes[0].cmp(&right.nodes[0]))
    });

    let mut facts = Vec::new();
    if verb == "run" {
        let mut by_code: Vec<(Option<i64>, usize)> = Vec::new();
        for group in &groups {
            match by_code
                .iter_mut()
                .find(|(code, _)| *code == group.exit_code)
            {
                Some((_, count)) => *count += group.nodes.len(),
                None => by_code.push((group.exit_code, group.nodes.len())),
            }
        }
        by_code.sort_by_key(|(code, _)| code.unwrap_or(i64::MAX));
        for (code, count) in by_code {
            facts.push(match code {
                Some(code) => format!("{count} exited {code}"),
                None => format!("{count} ok"),
            });
        }
    } else {
        let matched: usize = groups
            .iter()
            .filter(|group| !group.no_match)
            .map(|group| group.nodes.len())
            .sum();
        let no_match: usize = groups
            .iter()
            .filter(|group| group.no_match)
            .map(|group| group.nodes.len())
            .sum();
        if matched > 0 || no_match > 0 {
            facts.push(format!("{matched} matched"));
        }
        if no_match > 0 {
            facts.push(format!("{no_match} no match"));
        }
    }
    let mut by_status: Vec<(String, usize)> = Vec::new();
    for (_, status, _) in &failures {
        match by_status.iter_mut().find(|(known, _)| known == status) {
            Some((_, count)) => *count += 1,
            None => by_status.push((status.clone(), 1)),
        }
    }
    for (status, count) in by_status {
        facts.push(format!("{count} {status}"));
    }
    let subject = subject.unwrap_or_else(|| verb.to_string());
    let head = format!(
        "=== {subject} on {total} nodes ({total} nodes; {}) ===",
        facts.join("; ")
    );

    let mut lines = Vec::new();
    for (node, status, message) in &failures {
        if message.is_empty() {
            lines.push(format!("{node}: {status}"));
        } else {
            lines.push(format!("{node}: {status} ({message})"));
        }
    }
    if !images_omitted.is_empty() {
        lines.push(format!(
            "image content omitted for {}; inspect that machine alone to see it",
            images_omitted.join(", ")
        ));
    }
    let fixed = lines.join("\n");
    let mut used = estimate_tokens(&head) + estimate_tokens(&fixed) + 2;
    let mut headers = Vec::new();
    for group in &groups {
        let names = if group.nodes.len() > GROUP_NAMES_INLINE {
            format!(
                "{} (+{})",
                group.nodes[..GROUP_NAMES_INLINE].join(", "),
                group.nodes.len() - GROUP_NAMES_INLINE
            )
        } else {
            group.nodes.join(", ")
        };
        let header = if group.no_match {
            format!(
                "--- {} nodes: {names} --- ({})",
                group.nodes.len(),
                group.head_clauses
            )
        } else {
            format!(
                "--- {} nodes: {names} ---\n=== {subject} ({}) ===",
                group.nodes.len(),
                group.head_clauses
            )
        };
        used += estimate_tokens(&header) + 1;
        headers.push(header);
    }
    let available = budget_tokens.saturating_sub(used);
    let total_nodes_in_groups: usize = groups
        .iter()
        .filter(|group| !group.no_match)
        .map(|group| group.nodes.len())
        .sum::<usize>()
        .max(1);
    let mut sections = Vec::new();
    for (group, header) in groups.iter().zip(headers) {
        if group.no_match {
            sections.push(header);
            continue;
        }
        let share = available * group.nodes.len() / total_nodes_in_groups;
        let body = fit_lines(&group.body, share);
        if body.is_empty() {
            sections.push(header);
        } else {
            sections.push(format!("{header}\n{body}"));
        }
    }
    let mut body = String::new();
    if !fixed.is_empty() {
        body.push_str(&fixed);
    }
    for section in sections {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&section);
    }
    let text = if body.is_empty() {
        head
    } else {
        format!("{head}\n{body}")
    };
    ToolResponse::text(text)
}

/// Keeps as many leading lines as fit in `tokens`, noting what was cut.
fn fit_lines(body: &str, tokens: usize) -> String {
    if estimate_tokens(body) <= tokens {
        return body.to_string();
    }
    let mut kept = Vec::new();
    let mut used = 0;
    let note = "… (cut to fit the budget; inspect one machine alone for the rest)";
    let note_tokens = estimate_tokens(note) + 1;
    for line in body.lines() {
        let cost = estimate_tokens(line) + 1;
        if used + cost + note_tokens > tokens {
            break;
        }
        used += cost;
        kept.push(line);
    }
    kept.push(note);
    kept.join("\n")
}

/// The `nodes` tool body.
async fn list_nodes(client: &Arc<WorldClient>, request: NodesRequest) -> ToolResponse {
    let mut stale = None;
    if client.is_connected() {
        if let Err(error) = client.refresh_members().await {
            stale = Some(error);
        } else if let Err(error) = client.refresh_inventories().await {
            stale = Some(error);
        }
    } else {
        stale = Some(client.unreachable_error());
    }
    let all = client.nodes();
    let need = request.need.unwrap_or_default();
    let filtered = all
        .iter()
        .filter(|node| need.matches(node))
        .filter(|node| {
            request
                .tags
                .as_ref()
                .is_none_or(|tags| tags.iter().all(|tag| node.tags.contains(tag)))
        })
        .filter(|node| {
            request
                .online
                .is_none_or(|online| (node.state == "online") == online)
        })
        .collect::<Vec<_>>();
    let online = filtered
        .iter()
        .filter(|node| node.state == "online")
        .count();
    let filtering = !need.is_empty() || request.tags.is_some() || request.online.is_some();
    let mut note = if filtering {
        HeadNote::new(
            "nodes",
            HeadMetric::event(format!(
                "{} match; {} exist",
                counted(filtered.len(), "node", "nodes"),
                counted(all.len(), "node", "nodes")
            )),
        )
    } else {
        HeadNote::new("nodes", HeadMetric::count(filtered.len(), "node", "nodes"))
    };
    // With the hub unreachable this member cannot know who is online now, only who was. The
    // count says which of the two it is, because a bare "2 online" reads as a live fact even
    // when a later fact calls the whole answer cached.
    note = match &stale {
        Some(_) => note.fact(format!("{online} last known online")),
        None => note.fact(format!("{online} online")),
    };
    if let Some(stale) = stale {
        note = note.fact(format!("cached facts; {stale}"));
    }
    let mut lines = Vec::new();
    for node in filtered {
        lines.push(render_node(node));
    }
    note.into_text_response(&lines.join("\n"))
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn render_node(node: &NodeView) -> String {
    let mut columns = vec![
        format!("{:<14}", node.name),
        format!("{:<15}", format!("{}/{}", node.os, node.arch)),
        format!("{:<8}", node.state),
    ];
    if node.state == "online" {
        if let Some(inventory) = &node.inventory {
            columns.push(format!("cpus {:<3}", inventory.cpus));
            columns.push(format!("mem {}G", trim_float(inventory.memory_gb)));
            if !inventory.gpus.is_empty() {
                columns.push(format!(
                    "gpu {}",
                    inventory
                        .gpus
                        .iter()
                        .map(|gpu| format!("{} {}G", gpu.model, trim_float(gpu.memory_gb)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if inventory.wsl2 == Some(true) {
                columns.push("wsl2".to_string());
            }
        }
    } else {
        columns.push(format!(
            "last seen {}",
            if node.last_seen.is_empty() {
                "never"
            } else {
                &node.last_seen
            }
        ));
    }
    if !node.tags.is_empty() {
        columns.push(format!("tags {}", node.tags.join(",")));
    }
    let mut link = Vec::new();
    if let Some(network) = &node.network {
        link.push(network.clone());
    }
    if let Some(rtt) = node.hub_rtt_ms {
        link.push(format!("{rtt}ms"));
    }
    if !link.is_empty() {
        columns.push(format!("hub {}", link.join(" ")));
    }
    if node.is_self {
        columns.push("(this machine)".to_string());
    }
    columns.join("  ")
}

fn trim_float(value: f32) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::{NodeOutcome, fleet_response, parse_head, single_node_response};
    use crate::model::{ToolContent, ToolResponse};

    fn ok(node: &str, text: &str) -> NodeOutcome {
        NodeOutcome {
            node: node.to_string(),
            status: "ok".to_string(),
            response: Some(ToolResponse::text(text)),
            message: None,
        }
    }

    #[test]
    fn single_node_head_notes_name_the_machine_by_verb_shape() {
        let grep = single_node_response(
            "grep",
            ok(
                "linux-builder",
                "=== grep \"ERROR\" (matches 1-2 of 2) ===\na\nb",
            ),
        );
        assert_eq!(
            text(&grep).lines().next().unwrap(),
            "=== grep \"ERROR\" on linux-builder (matches 1-2 of 2) ==="
        );
        let file = single_node_response(
            "inspect_local_file",
            ok("win-test", "=== C:/repo/x (lines 1-3 of 3) ===\n1\tx"),
        );
        assert_eq!(
            text(&file).lines().next().unwrap(),
            "=== win-test:C:/repo/x (lines 1-3 of 3) ==="
        );
        assert!(
            parse_head("=== run (142 lines; exited 0) ===")
                .is_some_and(|head| head.clauses == "142 lines; exited 0")
        );
        assert!(parse_head("not a head").is_none());
    }

    #[test]
    fn fleet_groups_identical_outputs_and_lists_failures_first() {
        let outcomes = vec![
            ok("a", "=== grep \"x\" (matches 1-1 of 1) ===\nfoo"),
            ok("b", "=== grep \"x\" (matches 1-1 of 1) ===\nfoo"),
            ok("c", "=== grep \"x\" (0 matches) ==="),
            NodeOutcome {
                node: "d".to_string(),
                status: "offline".to_string(),
                response: None,
                message: Some("last seen 2 m ago".to_string()),
            },
        ];
        let response = fleet_response("grep", outcomes, 8_000);
        let text = text(&response);
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(
            lines[0],
            "=== grep \"x\" on 4 nodes (4 nodes; 2 matched; 1 no match; 1 offline) ==="
        );
        assert_eq!(lines[1], "d: offline (last seen 2 m ago)");
        assert_eq!(lines[2], "--- 2 nodes: a, b ---");
        assert_eq!(lines[3], "=== grep \"x\" (matches 1-1 of 1) ===");
        assert_eq!(lines[4], "foo");
        assert_eq!(lines[5], "--- 1 nodes: c --- (0 matches)");
    }

    fn text(response: &ToolResponse) -> String {
        match &response.content[0] {
            ToolContent::Text(text) => text.clone(),
            ToolContent::Image { .. } => panic!("expected text"),
        }
    }
}
