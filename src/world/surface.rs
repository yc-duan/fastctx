//! The World-mode tool surface: `node` on the file tools and `run`, the `nodes` machine map,
//! and head notes that name the machine a call ran on.
//!
//! A call without `node` takes exactly the 1.0 local path. A call with `node` naming this
//! machine runs locally too, but answers as a World response (its head note names the
//! node). Everything else goes through the node daemon's hub link. One call names one
//! machine; several machines are several calls, and the host runs them in parallel.

use crate::budget::{GLOBAL_TOKEN_BUDGET_ENV, tool_token_budget};
use crate::head_note::{HeadMetric, HeadNote};
use crate::model::{ToolContent, ToolResponse};
use crate::server::FastCtxServer;
use crate::server_support::into_mcp_result;
use crate::world::client::{NodeOutcome, NodeView, WorldClient};
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

/// The `node` parameter on the five node-taking tools.
pub(crate) const NODE_PARAMETER: &str =
    "The machine to run this on, as named by nodes. Omit for this machine.";
/// `run`'s `cwd` in World mode. Omitted, a local command runs in the session directory and a
/// remote one in the target's home directory; the model has no other way to learn the second.
const CWD_PARAMETER_WITH_NODE: &str = "Absolute path of the working directory. Omit for the session working directory, or for the home directory when node names another machine.";
/// What a list, `all`, or `tag:<tag>` gets: one machine per call, several calls for several.
pub(crate) const NODE_ONE_MACHINE: &str = "node names one machine per call; call once per machine.";
/// What a World call on an unenrolled or degraded engine says.
pub(crate) const NODE_SERVICE_NOT_RUNNING: &str =
    "The FastCtx node service is not running on this machine; run 'fastctx node status'.";
/// Default deadline for a remote read when the tool itself has no timeout parameter.
const DEFAULT_REMOTE_TIMEOUT: Duration = Duration::from_secs(120);

/// The arguments of a World-mode tool call: the base tool's own object plus the optional
/// `node`. The published schema is the base schema with `node` added, so this type only needs
/// to be an object.
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
    /// Splits the machine name off; `None` means this machine, the 1.0 path.
    pub(crate) fn split(self) -> Result<(Option<String>, serde_json::Value), String> {
        let mut map = self.0;
        let node = match map.remove("node") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(name)) => Some(one_machine(&name)?),
            Some(serde_json::Value::Array(_)) => return Err(NODE_ONE_MACHINE.to_string()),
            Some(other) => {
                return Err(format!(
                    "Invalid node value {other}: expected one machine name as listed by nodes."
                ));
            }
        };
        Ok((node, serde_json::Value::Object(map)))
    }
}

/// Accepts exactly one machine name. The group words of earlier drafts (`all`, `tag:<tag>`) and
/// a list written into the string get the same answer a JSON list gets.
fn one_machine(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(
            "node is empty. Omit it for this machine, or name one machine as listed by nodes."
                .to_string(),
        );
    }
    if name == "all"
        || name.starts_with("tag:")
        || name.contains(',')
        || name.contains(char::is_whitespace)
    {
        return Err(NODE_ONE_MACHINE.to_string());
    }
    Ok(name.to_string())
}

/// The World-mode input schema of a node-taking tool: the local schema plus `node`, and a
/// `cwd` description that says where a remote command runs when `cwd` is omitted.
pub(crate) fn world_input_schema(
    schema: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut schema = schema.clone();
    let properties = schema
        .entry("properties")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let serde_json::Value::Object(properties) = properties {
        properties.insert(
            "node".to_string(),
            serde_json::json!({ "type": "string", "description": NODE_PARAMETER }),
        );
        if let Some(serde_json::Value::Object(cwd)) = properties.get_mut("cwd") {
            cwd.insert(
                "description".to_string(),
                serde_json::Value::String(CWD_PARAMETER_WITH_NODE.to_string()),
            );
        }
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
        let (node, value) = match arguments.split() {
            Ok(split) => split,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        match node {
            None => match serde_json::from_value(value) {
                Ok(request) => self.local_inspect_local_file(request).await,
                Err(error) => invalid_arguments("inspect_local_file", error),
            },
            Some(node) => {
                self.world_call(
                    "inspect_local_file",
                    node,
                    value,
                    crate::budget::READ_TOKEN_BUDGET_ENV,
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
        let (node, value) = match arguments.split() {
            Ok(split) => split,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        match node {
            None => match serde_json::from_value(value) {
                Ok(request) => self.local_grep(request, context).await,
                Err(error) => invalid_arguments("grep", error),
            },
            Some(node) => {
                self.world_call("grep", node, value, crate::budget::GREP_TOKEN_BUDGET_ENV)
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
        let (node, value) = match arguments.split() {
            Ok(split) => split,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        match node {
            None => match serde_json::from_value(value) {
                Ok(request) => self.local_glob(request, context).await,
                Err(error) => invalid_arguments("glob", error),
            },
            Some(node) => {
                self.world_call("glob", node, value, crate::budget::GLOB_TOKEN_BUDGET_ENV)
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
        let (node, value) = match arguments.split() {
            Ok(split) => split,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        match node {
            None => match serde_json::from_value(value) {
                Ok(request) => self.local_replace(request).await,
                Err(error) => invalid_arguments("replace", error),
            },
            Some(node) => {
                self.world_call("replace", node, value, GLOBAL_TOKEN_BUDGET_ENV)
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
        let (node, value) = match arguments.split() {
            Ok(split) => split,
            Err(error) => return into_mcp_result(ToolResponse::error(error)),
        };
        match node {
            None => match serde_json::from_value(value) {
                Ok(request) => self.local_run(request, context).await,
                Err(error) => invalid_arguments("run", error),
            },
            Some(node) => {
                self.world_call("run", node, value, crate::budget::RUN_TOKEN_BUDGET_ENV)
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
    /// Runs `verb` on the one machine `node` names and renders the World response.
    async fn world_call(
        &self,
        verb: &str,
        node: String,
        args: serde_json::Value,
        budget_variable: &'static str,
    ) -> CallToolResult {
        let _activity = self.activity.request();
        let Some(client) = self.world.clone() else {
            return into_mcp_result(ToolResponse::error(NODE_SERVICE_NOT_RUNNING));
        };
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
        // This machine is a valid target even before the hub has ever listed it, so the member
        // table is only consulted for other machines.
        let outcome = if node == client.name() {
            run_on_this_machine(&node, verb, args, Arc::clone(&session), timeout).await
        } else {
            if let Err(error) = client.resolve_target(&node) {
                return into_mcp_result(ToolResponse::error(error));
            }
            match client.call(verb, &node, args, budget, None, timeout).await {
                Ok(outcome) => outcome,
                Err(error) => return into_mcp_result(ToolResponse::error(error)),
            }
        };
        let verb = verb.to_string();
        let status_shell = self.shell.clone();
        let response = tokio::task::spawn_blocking(move || {
            session.activate(|| {
                crate::background_status::BackgroundDecorator::new(
                    status_shell.background_status(None),
                    budget_variable,
                )
                .finish(single_node_response(&verb, outcome))
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
        grant_revision: 0,
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
