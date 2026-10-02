//! `ask_colleague` and `message_colleague` (`specs/world-colleagues.md`, "两个工具").
//!
//! Both act for the resident behind this MCP session. They are part of the World's surface,
//! except for a colleague: a colleague cannot ask for colleagues.

use super::text;
use crate::server::FastCtxServer;
use crate::town::core::Spoken;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub(crate) struct AskColleagueRequest {
    #[schemars(description = text::ASK_PROMPT)]
    pub(crate) prompt: String,
    #[schemars(description = text::ASK_DESCRIPTION)]
    pub(crate) description: String,
    #[schemars(description = text::ASK_NODE)]
    #[serde(default)]
    pub(crate) node: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub(crate) struct MessageColleagueRequest {
    #[schemars(description = text::MESSAGE_TO)]
    pub(crate) to: String,
    #[schemars(description = text::MESSAGE_TEXT)]
    pub(crate) message: String,
}

#[tool_router(router = colleague_tool_router, vis = "pub(crate)")]
impl FastCtxServer {
    #[tool(
        name = "ask_colleague",
        description = "Ask a colleague.",
        annotations(
            title = "Ask a colleague",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn ask_colleague(&self, Parameters(request): Parameters<AskColleagueRequest>) -> CallToolResult {
        let (town, requester) = match self.resident_for_tools() {
            Ok(found) => found,
            Err(error) => return failed(error),
        };
        if let Some(node) = request.node.as_deref().map(str::trim).filter(|node| !node.is_empty())
            && node != town.machine()
        {
            return failed(format!(
                "ask_colleague cannot start a colleague on {node} in this build; only this machine ({}) can host one.",
                town.machine()
            ));
        }
        match town.ask_colleague(&requester, &request.prompt, request.description.trim()).await {
            Ok(asked) => CallToolResult::success(vec![ContentBlock::text(text::colleague_started(&asked.id, &asked.machine))]),
            Err(error) => failed(error),
        }
    }

    #[tool(
        name = "message_colleague",
        description = "Speak to a colleague.",
        annotations(
            title = "Speak to a colleague",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn message_colleague(&self, Parameters(request): Parameters<MessageColleagueRequest>) -> CallToolResult {
        let (town, from) = match self.resident_for_tools() {
            Ok(found) => found,
            Err(error) => return failed(error),
        };
        let to = request.to.trim();
        match town.speak(&from, to, &request.message).await {
            Ok(spoken) => {
                let how = match spoken {
                    Spoken::NewTurn => "started a new turn",
                    Spoken::JoinedTurn => "joined its current turn",
                    Spoken::Queued => "waiting for it to read",
                };
                CallToolResult::success(vec![ContentBlock::text(text::message_landed(to, town.machine(), how))])
            }
            Err(error) => failed(error),
        }
    }
}

/// The colleague tools with their published descriptions; the attribute only takes literals.
pub(crate) fn router() -> ToolRouter<FastCtxServer> {
    let mut router = FastCtxServer::colleague_tool_router();
    for (name, description) in [
        ("ask_colleague", text::ASK_COLLEAGUE_DESCRIPTION),
        ("message_colleague", text::MESSAGE_COLLEAGUE_DESCRIPTION),
    ] {
        router.map.get_mut(name).expect("the colleague router contains both tools").attr.description = Some(description.into());
    }
    router
}

fn failed(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}
