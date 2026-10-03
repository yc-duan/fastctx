//! The World's surface on an enrolled machine: `run`, one of the two ways to write, the machine
//! map and `copy`, and the colleague tools (`specs/world-run.md`, `world-write.md`,
//! `world-colleagues.md`). It replaces the 1.0 surface whatever that enables; every tool acts
//! for the resident behind this session, on this machine or on the one `node` names.

use super::Starter;
use super::run::{self as hand_run, Runner};
use super::text;
use super::write::{self as hand_write, Writer as WriteHand};
use crate::server::{FastCtxServer, ServerOptions};
use crate::server_manifest::{ToolManifest, world_entry_published};
use crate::server_support::into_mcp_result;
use crate::town::core::Town;
use crate::write::{ApplyPatchRequest, EditRequest, WriteError, WriteRequest, WriteResult};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunArguments {
    #[schemars(description = text::RUN_COMMAND)]
    command: String,
    #[schemars(description = text::RUN_CWD)]
    #[serde(default)]
    cwd: Option<String>,
    #[schemars(description = text::NODE)]
    #[serde(default)]
    node: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditArguments {
    #[schemars(description = text::EDIT_PATH)]
    file_path: String,
    #[schemars(description = text::EDIT_OLD)]
    old_string: String,
    #[schemars(description = text::EDIT_NEW)]
    new_string: String,
    #[schemars(description = text::EDIT_ALL)]
    #[serde(default)]
    replace_all: bool,
    #[schemars(description = text::NODE)]
    #[serde(default)]
    node: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteArguments {
    #[schemars(description = text::WRITE_PATH)]
    file_path: String,
    #[schemars(description = text::WRITE_CONTENT)]
    content: String,
    #[schemars(description = text::NODE)]
    #[serde(default)]
    node: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApplyPatchArguments {
    #[schemars(description = text::APPLY_PATCH_INPUT)]
    input: String,
    #[schemars(description = text::NODE)]
    #[serde(default)]
    node: Option<String>,
}

#[tool_router(router = hands_tool_router, vis = "pub(crate)")]
impl FastCtxServer {
    #[tool(
        name = "run",
        description = "Run a bash command.",
        annotations(
            title = "Run a bash command",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn hands_run(&self, Parameters(arguments): Parameters<RunArguments>) -> CallToolResult {
        let _activity = self.activity.request();
        match self.reach(arguments.node.as_deref()) {
            Err(failure) => failure,
            Ok(Reach::Here(town, resident)) => {
                let runner = Runner {
                    town,
                    starter: Starter::Here(resident),
                };
                match hand_run::run(
                    &runner,
                    &self.session.command_environment,
                    arguments.command,
                    arguments.cwd,
                )
                .await
                {
                    Ok(result) => into_mcp_result(hand_run::response(&result)),
                    Err(error) => failed(error),
                }
            }
            Ok(Reach::There(town, resident, node)) => {
                let (what, _) = hand_run::action(&arguments.command, &Err(String::new()));
                let request =
                    serde_json::json!({ "command": arguments.command, "cwd": arguments.cwd });
                into_mcp_result(
                    town.hand_elsewhere(&resident, &node, "run", &what, request)
                        .await,
                )
            }
        }
    }

    #[tool(
        name = "edit",
        description = "Edit a file.",
        annotations(
            title = "Edit a file",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn hands_edit(&self, Parameters(arguments): Parameters<EditArguments>) -> CallToolResult {
        let request = EditRequest {
            file_path: arguments.file_path,
            old_string: arguments.old_string,
            new_string: arguments.new_string,
            replace_all: arguments.replace_all,
        };
        let what = request.file_path.clone();
        let wire = serde_json::json!(request);
        self.write_hand(arguments.node.as_deref(), "edit", what, wire, move |hand| {
            hand_write::edit(hand, request)
        })
        .await
    }

    #[tool(
        name = "write",
        description = "Write a file.",
        annotations(
            title = "Write a file",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn hands_write(
        &self,
        Parameters(arguments): Parameters<WriteArguments>,
    ) -> CallToolResult {
        let request = WriteRequest {
            file_path: arguments.file_path,
            content: arguments.content,
        };
        let what = request.file_path.clone();
        let wire = serde_json::json!(request);
        self.write_hand(
            arguments.node.as_deref(),
            "write",
            what,
            wire,
            move |hand| hand_write::write(hand, request),
        )
        .await
    }

    #[tool(
        name = "apply_patch",
        description = "Apply a patch.",
        annotations(
            title = "Apply a patch",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn hands_apply_patch(
        &self,
        Parameters(arguments): Parameters<ApplyPatchArguments>,
    ) -> CallToolResult {
        let request = ApplyPatchRequest {
            input: arguments.input,
        };
        let what = hand_write::patch_files(&request.input);
        let wire = serde_json::json!(request);
        self.write_hand(
            arguments.node.as_deref(),
            "apply_patch",
            what,
            wire,
            move |hand| hand_write::apply_patch(hand, request),
        )
        .await
    }
}

/// Where a hand acts for the resident behind this session: on this machine, or on another.
enum Reach {
    Here(Arc<Town>, String),
    There(Arc<Town>, String, String),
}

impl FastCtxServer {
    /// The town and the resident a hand acts for, and the machine `node` names.
    fn reach(&self, node: Option<&str>) -> Result<Reach, CallToolResult> {
        let (town, resident) = self.resident_for_tools().map_err(failed)?;
        Ok(match node.map(str::trim).filter(|node| !node.is_empty()) {
            Some(node) if node != town.machine() => Reach::There(town, resident, node.to_string()),
            _ => Reach::Here(town, resident),
        })
    }

    /// A write hand: run here, or `wire` sent to the machine `node` names.
    async fn write_hand(
        &self,
        node: Option<&str>,
        hand: &str,
        what: String,
        wire: serde_json::Value,
        operation: impl FnOnce(&WriteHand) -> Result<WriteResult, WriteError> + Send + 'static,
    ) -> CallToolResult {
        let _activity = self.activity.request();
        let (town, resident) = match self.reach(node) {
            Err(failure) => return failure,
            Ok(Reach::There(town, resident, node)) => {
                return into_mcp_result(
                    town.hand_elsewhere(&resident, &node, hand, &what, wire)
                        .await,
                );
            }
            Ok(Reach::Here(town, resident)) => (town, resident),
        };
        let outcome = tokio::task::spawn_blocking(move || {
            operation(&WriteHand::new(town, Starter::Here(resident)))
        })
        .await;
        match outcome {
            Ok(Ok(result)) => CallToolResult::success(vec![ContentBlock::text(result.text)]),
            Ok(Err(error)) => failed(error.to_string()),
            Err(error) => failed(format!("The write stopped: {error}")),
        }
    }
}

/// The surface one World configuration publishes, with descriptions the attribute cannot take.
pub(crate) fn router(options: ServerOptions) -> ToolRouter<FastCtxServer> {
    let mut router = FastCtxServer::hands_tool_router();
    for (name, description) in [
        ("run", text::RUN_DESCRIPTION),
        ("edit", text::EDIT_DESCRIPTION),
        ("write", text::WRITE_DESCRIPTION),
        ("apply_patch", text::APPLY_PATCH_DESCRIPTION),
    ] {
        router
            .map
            .get_mut(name)
            .expect("the hands router contains every hand")
            .attr
            .description = Some(description.into());
    }
    // Claude Code keeps a tool result up to this many characters before setting it aside in a
    // file; one run never gives more (`specs/world-view.md`, "一次给多少").
    let mut meta = serde_json::Map::new();
    meta.insert(
        "anthropic/maxResultSizeChars".to_string(),
        serde_json::json!(crate::run::CHARACTER_BUDGET),
    );
    router
        .map
        .get_mut("run")
        .expect("the hands router contains run")
        .attr
        .meta = Some(rmcp::model::Meta(meta));
    let mut machines = FastCtxServer::world_tool_router();
    for name in ["nodes", "copy"] {
        router.add_route(
            machines
                .map
                .remove(name)
                .expect("the World router contains nodes and copy"),
        );
    }
    router.merge(super::colleague::router());
    for entry in ToolManifest::world_entries() {
        if !world_entry_published(entry.name, options) {
            router.remove_route(entry.name);
        }
    }
    router
}

fn failed(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}
