//! The machine map `nodes` and `copy` between machines, the two World tools that are not
//! hands, and the head notes that name the machine a call ran on.

use crate::budget::{GLOBAL_TOKEN_BUDGET_ENV, tool_token_budget};
use crate::head_note::{HeadMetric, HeadNote};
use crate::model::{ToolContent, ToolResponse};
use crate::server::FastCtxServer;
use crate::server_support::into_mcp_result;
use crate::world::client::{NodeOutcome, NodeView, WorldClient};
use crate::world::dispatch::{NodeCall, call_node};
use crate::world::messages::{CallBudget, CallOrigin};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

/// What a list, `all`, or `tag:<tag>` gets: one machine per call, several calls for several.
pub(crate) const NODE_ONE_MACHINE: &str = "node names one machine per call; call once per machine.";
/// What a World call on an unenrolled or degraded engine says.
pub(crate) const NODE_SERVICE_NOT_RUNNING: &str =
    "The FastCtx node service is not running on this machine; run 'fastctx node status'.";
/// `copy`'s default when `timeout_ms` is omitted: the host's own ceiling for a tool call.
pub(crate) const DEFAULT_COPY_TIMEOUT_MS: u64 = 240_000;
const COPY_NEEDS_TWO_MACHINES: &str =
    "copy moves a file or directory between two machines; name from_node or to_node.";
const COPY_SAME_MACHINE: &str =
    "from_node and to_node name the same machine; copy works between two machines.";

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

/// Requirements a machine must meet.
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
        name = "nodes",
        description = "List the machines in this World: name, OS and architecture, online state, CPUs, memory, GPUs, tags, home directory and user, and how each is linked to the hub. Pass need to keep only machines that meet requirements (os, arch, cpus, memory_gb, gpus, gpu_memory_gb, tags, nodes); the first line reports how many matched out of how many exist. Machine names are the values other tools accept in node.",
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

    #[tool(
        name = "copy",
        description = "Copy a file or a directory tree between two machines in this World. Files already identical at to are skipped, differing ones replaced, directories merged, nothing at to deleted. The response reports counts, bytes and the SHA-256 of a single file, never contents.",
        annotations(
            title = "Copy between machines",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn copy(&self, Parameters(request): Parameters<CopyRequest>) -> CallToolResult {
        let Some(client) = self.world.clone() else {
            return into_mcp_result(ToolResponse::error(NODE_SERVICE_NOT_RUNNING));
        };
        let me = client.name();
        let named = |value: Option<String>| value.map(|name| one_machine(&name)).transpose();
        let (from_node, to_node) = match (named(request.from_node), named(request.to_node)) {
            (Ok(from_node), Ok(to_node)) => (from_node, to_node),
            (Err(error), _) | (_, Err(error)) => {
                return into_mcp_result(ToolResponse::error(error));
            }
        };
        if from_node.is_none() && to_node.is_none() {
            return into_mcp_result(ToolResponse::error(COPY_NEEDS_TWO_MACHINES));
        }
        let from_node = from_node.unwrap_or_else(|| me.clone());
        let to_node = to_node.unwrap_or_else(|| me.clone());
        if from_node == to_node {
            return into_mcp_result(ToolResponse::error(COPY_SAME_MACHINE));
        }
        let args = match serde_json::to_value(crate::world::copy::CopyArgs {
            from: request.from,
            to: request.to,
            to_node,
            caller: me,
            timeout_ms: Some(request.timeout_ms.unwrap_or(DEFAULT_COPY_TIMEOUT_MS)),
        }) {
            Ok(args) => args,
            Err(error) => {
                return into_mcp_result(ToolResponse::error(format!(
                    "Internal tool failure: {error}"
                )));
            }
        };
        self.world_call("copy", from_node, args, GLOBAL_TOKEN_BUDGET_ENV)
            .await
    }
}

/// Parameters of `copy`.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CopyRequest {
    /// Path of the file or directory to copy, on from_node.
    pub from: String,
    /// Path the copy will have on to_node: the file itself, or the directory that receives the tree. Missing parent directories are created.
    pub to: String,
    /// Machine holding from, as named by nodes. Omit for this machine.
    pub from_node: Option<String>,
    /// Machine receiving the copy, as named by nodes. Omit for this machine.
    pub to_node: Option<String>,
    /// Stop after this many milliseconds; finished files stay and the unfinished one is discarded, so running the same copy again continues it. Default 240000.
    #[schemars(range(min = 1, max = 240_000))]
    pub timeout_ms: Option<u64>,
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
        match self.world_outcome(verb, node, args, budget_variable).await {
            Ok(outcome) => self.world_finish(verb, outcome, budget_variable).await,
            Err(failure) => failure,
        }
    }

    async fn world_outcome(
        &self,
        verb: &str,
        node: String,
        args: serde_json::Value,
        budget_variable: &'static str,
    ) -> Result<NodeOutcome, CallToolResult> {
        let _activity = self.activity.request();
        let Some(client) = self.world.clone() else {
            return Err(into_mcp_result(ToolResponse::error(
                NODE_SERVICE_NOT_RUNNING,
            )));
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
        let timeout = args
            .get("timeout_ms")
            .and_then(serde_json::Value::as_u64)
            .map_or(
                Duration::from_millis(DEFAULT_COPY_TIMEOUT_MS),
                Duration::from_millis,
            );
        let call = NodeCall {
            verb: verb.to_string(),
            node,
            args,
            budget,
            timeout,
            origin: CallOrigin::Tool,
        };
        call_node(&client, session, call)
            .await
            .map_err(|error| into_mcp_result(ToolResponse::error(error)))
    }

    async fn world_finish(
        &self,
        verb: &str,
        outcome: NodeOutcome,
        budget_variable: &'static str,
    ) -> CallToolResult {
        let session = Arc::clone(&self.session);
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

/// The subject a World response shows for one machine.
fn node_subject(verb: &str, subject: &str, node: &str) -> String {
    match verb {
        // `copy` renders both of its endpoints itself.
        "copy" => subject.to_string(),
        _ => format!("{subject} on {node}"),
    }
}

/// Renders one machine's outcome as a World response whose head note names the machine.
pub(crate) fn single_node_response(verb: &str, outcome: NodeOutcome) -> ToolResponse {
    match outcome.response {
        Some(mut response) if !response.is_error => {
            if let Some(ToolContent::Text(text)) = response.content.first_mut() {
                let updated = {
                    let (first, rest) = text
                        .split_once('\n')
                        .map_or((text.as_str(), None), |(first, rest)| (first, Some(rest)));
                    let head = match crate::head_note::split_rendered(first) {
                        Some((subject, clauses)) => format!(
                            "=== {} ({clauses}) ===",
                            node_subject(verb, subject, &outcome.node)
                        ),
                        None => format!("[{}] {first}", outcome.node),
                    };
                    match rest {
                        Some(rest) => format!("{head}\n{rest}"),
                        None => head,
                    }
                };
                *text = updated;
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
    let mut all = client.nodes();
    if let Err(error) = super::notes::annotate(&client.paths, &mut all) {
        return ToolResponse::error(error);
    }
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
    if let Some(inventory) = &node.inventory {
        if !inventory.home.is_empty() {
            columns.push(format!("home {}", inventory.home));
        }
        if !inventory.user.is_empty() {
            columns.push(format!("user {}", inventory.user));
        }
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
    if let Some(note) = node.effective_note() {
        columns.push(format!("note {}", super::notes::quoted(note)));
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
